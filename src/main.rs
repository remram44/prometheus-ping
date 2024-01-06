use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use clap::{App, Arg};
use hyper::header::CONTENT_TYPE;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use prometheus::{Encoder, TextEncoder};
use prometheus::core::{Collector, Desc};
use prometheus::proto::MetricFamily;
use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SAMPLES: usize = 30;

fn current_time(buf: &mut [u8; 8]) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let secs = now.as_secs();
    let nsecs = now.subsec_nanos();
    let mut cursor = Cursor::new(buf as &mut [u8]);
    cursor.write_u32::<BigEndian>(secs as u32).unwrap();
    cursor.write_u32::<BigEndian>(nsecs as u32).unwrap();
}

fn get_delay(buf: &[u8; 8]) -> Duration {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let mut cursor = Cursor::new(buf);
    let then = Duration::new(
        cursor.read_u32::<BigEndian>().unwrap() as u64,
        cursor.read_u32::<BigEndian>().unwrap() as u32,
    );
    now - then
}

fn main() {
    let cli = App::new("prometheus-ping")
        .bin_name("prometheus-ping")
        .version(env!("CARGO_PKG_VERSION"))
        .author(env!("CARGO_PKG_AUTHORS"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .arg(
            Arg::with_name("TARGET")
                .takes_value(true)
                .multiple(true)
        )
        .arg(
            Arg::with_name("metrics-port")
                .long("metrics-port")
                .takes_value(true)
                .default_value("8000")
        )
        .arg(
            Arg::with_name("listen-port")
                .long("listen-port")
                .takes_value(true)
                .default_value("5000")
        )
        .arg(
            Arg::with_name("source")
                .long("source")
                .takes_value(true)
                .required(true)
        );
    let matches = cli.get_matches();

    let metrics_port: u16 = match matches.value_of("metrics-port").unwrap().parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Invalid metrics port");
            exit(1);
        }
    };

    let listen_port: u16 = match matches.value_of("listen-port").unwrap().parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Invalid listen port");
            exit(1);
        }
    };

    let source = matches.value_of("source").unwrap();
    eprintln!("Source: {}", source);

    // Resolve targets
    eprintln!("Targets:");
    let mut targets: HashMap<SocketAddr, String> = HashMap::new();
    if let Some(l) = matches.values_of("TARGET") {
        for target in l {
            let addr: SocketAddr = match target.to_socket_addrs() {
                Ok(mut i) => match i.next() {
                    Some(a) => a,
                    None => {
                        eprintln!("No address for target {}", target);
                        exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Invalid target {}: {}", target, e);
                    exit(1);
                }
            };
            eprintln!("  {} -> {}", target, addr);
            targets.insert(addr, target.to_owned());
        }
    }
    if targets.is_empty() {
        eprintln!("  no targets");
    }
    let targets = Arc::new(targets);

    // Create echo server (for others to ping)
    eprintln!("Starting UDP echo server on port {}", listen_port);
    let echo_server = match UdpSocket::bind(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
        listen_port,
    )) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Can't create echo server on port {}: {}",
                listen_port,
                e,
            );
            exit(1);
        }
    };
    thread::spawn(move || {
        let mut buf = [0; 8];
        loop {
            let (len, src) = echo_server.recv_from(&mut buf).expect("echo server recv");

            if let Err(e) = echo_server.send_to(&buf[..len], &src) {
                eprintln!(
                    "Error sending {}-bytes reply to {}: {}",
                    len,
                    src,
                    e,
                );
            }
        }
    });

    // Create local socket (to ping)
    let ping_socket = Arc::new(UdpSocket::bind("0.0.0.0:0").unwrap());

    // Setup Prometheus exposition
    let mut targets_info: HashMap<String, TargetInfo> = HashMap::new();
    for target in targets.values() {
        targets_info.insert(target.to_owned(), Default::default());
    }
    let targets_info = Arc::new(Mutex::new(targets_info));
    let collector = PingCollector::new(
        source.to_owned(),
        targets_info.clone(),
    );
    prometheus::register(Box::new(collector)).expect("register collector");

    // Setup metrics server with Tokio and Hyper
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().worker_threads(1).build().unwrap();
        rt.block_on(async {
            eprintln!("Starting metrics server on port {}", metrics_port);
            let metrics_server = Server::bind(&SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
                metrics_port,
            ));
            metrics_server.serve(make_service_fn(|_| async {
                Ok::<_, hyper::Error>(service_fn(serve_req))
            })).await.unwrap();
            exit(1);
        });
    });

    // Create ping thread
    let t = targets.clone();
    let ti = targets_info.clone();
    let ps = ping_socket.clone();
    thread::spawn(move || {
        let targets = &*t;
        let targets_info = &*ti;
        let ping_socket = &*ps;
        let mut buf = [0; 8];
        loop {
            thread::sleep(Duration::from_secs(1));
            let mut map = targets_info.lock().unwrap();
            for (addr, target) in targets {
                current_time(&mut buf);
                if let Err(e) = ping_socket.send_to(&buf, addr) {
                    eprintln!(
                        "Error sending {}-bytes request to {}: {}",
                        buf.len(),
                        addr,
                        e,
                    );
                }
                let target_info = map.get_mut(target).unwrap();
                target_info.send_counter += 1;
                target_info.in_flight = 1;
            }
        }
    });

    // Receive replies and compute metrics
    {
        let targets = &*targets;
        let targets_info = &*targets_info;
        let ping_socket = &*ping_socket;
        let mut buf = [0; 8];
        loop {
            let (len, src) = ping_socket.recv_from(&mut buf).expect("ping send");
            if len != 8 {
                continue;
            }
            let target = match targets.get(&src) {
                Some(t) => t,
                None => continue,
            };
            let delay = get_delay(&buf);
            let mut map = targets_info.lock().unwrap();
            let target_info = map.get_mut(target).unwrap();
            target_info.recv_counter += 1;
            target_info.in_flight = 0;
            if target_info.latencies.len() + 1 > SAMPLES {
                target_info.latencies.truncate(SAMPLES - 1);
            }
            target_info.latencies.push_front((SystemTime::now(), delay.as_secs_f64()));
        }
    }
}

#[derive(Default)]
struct TargetInfo {
    send_counter: u64,
    recv_counter: u64,
    latencies: VecDeque<(SystemTime, f64)>,
    in_flight: u64,
}

struct PingCollector {
    source: String,
    targets_info: Arc<Mutex<HashMap<String, TargetInfo>>>,
    desc: Vec<&'static Desc>,
}

impl PingCollector {
    fn new(source: String, targets_info: Arc<Mutex<HashMap<String, TargetInfo>>>) -> PingCollector {
        struct TempDesc {
            name: &'static str,
            descr: &'static str,
            labels: &'static [&'static str],
        }
        impl TempDesc {
            fn new(name: &'static str, descr: &'static str, labels: &'static [&'static str]) -> TempDesc {
                TempDesc {
                    name,
                    descr,
                    labels,
                }
            }
        }
        let desc = &[
            TempDesc::new(
                "ping_packet_loss_total",
                "Lost packets",
                &["source", "target"],
            ),
            TempDesc::new(
                "ping_latency_average_30s",
                "Average round-trip latency over the last 30s",
                &["source", "target"],
            ),
        ];
        let desc = desc.into_iter()
            .map(|e| Desc::new(
                e.name.to_owned(),
                e.descr.to_owned(),
                e.labels.iter().map(|s: &&str| (*s).to_owned()).collect(),
                HashMap::new(),
            ).unwrap())
            .map(|e| Box::leak(Box::new(e)) as &_)
            .collect();

        PingCollector {
            source,
            targets_info,
            desc,
        }
    }
}

impl Collector for PingCollector {
    fn desc(&self) -> Vec<&Desc> {
        self.desc.clone()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let now = SystemTime::now();
        let targets_info = &*self.targets_info.lock().unwrap();

        let packet_loss = prometheus::IntCounterVec::new(
            prometheus::Opts::new("ping_packet_loss_total", "Lost packets"),
            &["source", "target"],
        ).unwrap();

        let latency_avg = prometheus::GaugeVec::new(
            prometheus::Opts::new("ping_latency_average_30s", "Average round-trip latency over the last 30s"),
            &["source", "target"],
        ).unwrap();

        for (target, target_info) in targets_info {
            let loss = target_info.send_counter - target_info.recv_counter - target_info.in_flight;
            let m = packet_loss.with_label_values(&[&self.source, target]);
            m.reset();
            m.inc_by(loss);

            let mut latency = 0.0;
            let mut count = 0;
            for &(when, lat) in &target_info.latencies {
                if when < now - Duration::from_secs(SAMPLES as u64) {
                    // Too old, stop
                    break;
                }

                latency += lat;
                count += 1;
            }
            if count > 0 {
                latency /= count as f64;
                latency_avg.with_label_values(&[&self.source, target]).set(latency);
            }
        }

        let mut result = Vec::new();
        result.extend(packet_loss.collect());
        result.extend(latency_avg.collect());
        result
    }
}

async fn serve_req(_req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    let encoder = TextEncoder::new();

    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer).unwrap();

    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(Body::from(buffer))
        .unwrap();

    Ok(response)
}
