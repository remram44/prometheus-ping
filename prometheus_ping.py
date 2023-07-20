import argparse
import logging
import prometheus_client
import socket
import struct
import threading
import time


__version__ = '0.1.0'


logger = logging.getLogger('prometheus_ping')


SAMPLES = 30


def current_time():
    """Return perf_counter_ns() as a 64-bit binary buffer.
    """
    return struct.pack('>Q', time.perf_counter_ns())


def get_delay(buf):
    """Return the elapsed time from the given message.
    """
    now = time.perf_counter_ns()
    orig, = struct.unpack('>Q', buf)
    return now - orig


def parse_target(target):
    host, port = target.split(':')
    params = socket.getaddrinfo(host, port, socket.AF_INET, socket.SOCK_DGRAM)
    return params[0][4]


def main():
    logging.basicConfig(level=logging.INFO)

    parser = argparse.ArgumentParser(
        'prometheus_ping',
        description=(
            "Measure ping between locations and report as Prometheus metrics"
        ),
    )
    parser.add_argument('--metrics-port', default=8000, type=int)
    parser.add_argument('--listen-port', default=5000, type=int)
    parser.add_argument('--source', required=True)
    parser.add_argument('target', nargs='*')
    args = parser.parse_args()

    logger.info("Starting prometheus_ping %s", __version__)
    logger.info("Source: %s", args.source)

    # Resolve targets
    logger.info("Targets:")
    targets = {}
    for target in args.target:
        addr = parse_target(target)
        targets[addr] = target
        logger.info("  %s -> %s:%s", target, addr[0], addr[1])
    if not targets:
        logger.info("  no targets")

    # Create echo server (for others to ping)
    logger.info("Starting UDP echo server on port %d", args.listen_port)
    echo_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    echo_sock.bind(('0.0.0.0', args.listen_port))
    server_thread = threading.Thread(
        target=server,
        args=[echo_sock],
        name="UDP echo server",
        daemon=True,
    )
    server_thread.start()

    # Create local socket (to ping)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(('0.0.0.0', 0))

    # Setup Prometheus expositing
    collector = Collector(args.source, targets)
    prometheus_client.REGISTRY.register(collector)
    logger.info("Starting Prometheus server on port %d", args.metrics_port)
    prometheus_client.start_wsgi_server(args.metrics_port)

    # Create ping thread
    ping_thread = threading.Thread(
        target=ping,
        args=[sock, targets, collector],
        name="Ping sender",
        daemon=True,
    )
    ping_thread.start()

    # Receive replies and compute metrics
    receive(sock, targets, collector)


def server(sock):
    while True:
        data, addr = sock.recvfrom(1024)
        sock.sendto(data, addr)


def ping(sock, targets, collector):
    while True:
        time.sleep(1)
        for (addr, target) in targets.items():
            sock.sendto(current_time(), addr)
            collector.send_counters[target] += 1
            collector.in_flight[target] = 1


class Collector(object):
    def __init__(self, source, targets):
        self.source = source
        self.send_counters = {target: 0 for target in targets.values()}
        self.recv_counters = {target: 0 for target in targets.values()}
        self.latencies = {target: [] for target in targets.values()}
        self.in_flight = {target: 0 for target in targets.values()}

    def collect(self):
        m_packet_loss = prometheus_client.metrics_core.CounterMetricFamily(
            'ping_packet_loss', "Lost packets",
            labels=['source', 'target'],
        )

        # Packet loss is the number of queries minus the number of replies
        # However we have to account for the last reply which might be on the
        # way
        # Unless we got a reply less than a second ago, discount one
        for target, sent in self.send_counters.items():
            received = self.recv_counters[target]
            loss = sent - received
            loss -= self.in_flight[target]
            m_packet_loss.add_metric([self.source, target], loss)

        m_latency_avg = prometheus_client.metrics_core.GaugeMetricFamily(
            'ping_latency_average_30s', "Average round-trip latency over the last 30s",
            labels=['source', 'target'],
        )

        for target, latencies in self.latencies.items():
            if latencies:
                latency = sum(latencies) / len(latencies) / 1e9
                m_latency_avg.add_metric([self.source, target], latency)

        return [
            m_packet_loss,
            m_latency_avg,
        ]


def receive(sock, targets, collector):
    while True:
        data, addr = sock.recvfrom(1024)
        try:
            target = targets[addr]
        except KeyError:
            continue
        collector.recv_counters[target] += 1
        collector.in_flight[target] = 0
        delay = get_delay(data)
        latencies = collector.latencies[target]
        latencies.append(delay)
        if len(latencies) > SAMPLES:
            latencies[:] = latencies[-SAMPLES:]


if __name__ == '__main__':
    main()
