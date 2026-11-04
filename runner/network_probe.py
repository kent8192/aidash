"""Network admission probes executed with the untrusted user's capabilities."""
import ipaddress
import json
import socket
import struct
import sys


def _checksum(data):
    if len(data) % 2:
        data += b'\0'
    value = sum(struct.unpack('!' + 'H' * (len(data) // 2), data))
    value = (value & 0xffff) + (value >> 16)
    value = (value & 0xffff) + (value >> 16)
    return (~value) & 0xffff


def spoof_packet(source, destination, port, payload):
    src, dst = socket.inet_aton(source), socket.inet_aton(destination)
    udp = struct.pack('!HHHH', 49152, port, 8 + len(payload), 0) + payload
    header = struct.pack('!BBHHHBBH4s4s', 0x45, 0, 20 + len(udp), 1, 0, 64,
                         socket.IPPROTO_UDP, 0, src, dst)
    checksum = _checksum(header)
    header = header[:10] + struct.pack('!H', checksum) + header[12:]
    return header + udp


def _connect(address, port):
    ipaddress.ip_address(address)
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError('invalid probe port')
    try:
        with socket.create_connection((address, port), timeout=2):
            return True
    except OSError:
        return False


def probe(targets, docker):
    report = {}
    for name in ('gateway', 'peer', 'node', 'metadata', 'internet'):
        target = targets[name]
        report[name] = _connect(target['address'], target['port'])
    report['raw_sent'] = False
    if docker:
        peer, node = targets['peer'], targets['node']
        payload = bytes.fromhex(targets['nonce'])
        if len(payload) != 16:
            raise ValueError('invalid spoof probe nonce')
        packets = [(peer['address'], targets['udp_port'], peer['address']),
                   (node['address'], node['port'], node['address']),
                   (peer['address'], targets['udp_port'], targets['gateway']['address']),
                   (node['address'], node['port'], targets['gateway']['address'])]
        with socket.socket(socket.AF_INET, socket.SOCK_RAW, socket.IPPROTO_RAW) as raw:
            raw.setsockopt(socket.IPPROTO_IP, socket.IP_HDRINCL, 1)
            report['raw_sent'] = all(
                raw.sendto(packet, (route, port)) == len(packet)
                for destination, port, route in packets
                for packet in [spoof_packet(targets['gateway']['address'], destination, port, payload)])
    return report


if __name__ == '__main__':
    if len(sys.argv) != 3 or sys.argv[1] not in ('normal', 'docker'):
        raise SystemExit('invalid network probe request')
    print(json.dumps(probe(json.loads(sys.argv[2]), sys.argv[1] == 'docker')))
