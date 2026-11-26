"""SFTP read fixture run over stdin/stdout by the Rust protocol tests."""

from collections import deque
from queue import Empty, Queue
from threading import Thread
import struct
import sys
import time


mode, size, window, chunk_size = sys.argv[1:]
size, window, chunk_size = int(size), int(window), int(chunk_size)
pattern = bytes(range(251))
payload = (pattern * ((size + len(pattern) - 1) // len(pattern)))[:size]
handle = b"file"
attrs = struct.pack(">IQI", 5, size, 0o100644)  # size and permissions
incoming = Queue()


def receive_requests():
    reader = sys.stdin.buffer
    while length := reader.read(4):
        packet = reader.read(struct.unpack(">I", length)[0])
        incoming.put(packet)
    incoming.put(None)


def send(packet):
    writer = sys.stdout.buffer
    writer.write(struct.pack(">I", len(packet)))
    writer.write(packet)
    writer.flush()


def reply(kind, request_id, body):
    send(struct.pack(">BI", kind, request_id) + body)


def string(data):
    return struct.pack(">I", len(data)) + data


def read_reply(request_id, offset, count):
    reply(103, request_id, string(payload[offset:offset + count]))


Thread(target=receive_requests, daemon=True).start()
initial = [] if mode in ("reorder", "duplicate") else None
delayed = deque()
while True:
    while delayed and delayed[0][0] <= time.monotonic():
        _, read = delayed.popleft()
        read_reply(*read)
    if delayed:
        timeout = max(0, delayed[0][0] - time.monotonic())
    else:
        timeout = 2 if initial else None
    try:
        packet = incoming.get(timeout=timeout)
    except Empty:
        if not delayed:
            raise RuntimeError("client did not send the complete initial read window")
        continue
    if packet is None:
        break
    kind, request_id = struct.unpack_from(">BI", packet)
    if kind == 1:  # INIT has a version field in place of the request ID.
        assert request_id == 3
        send(b"\x02\x00\x00\x00\x03")
    elif kind in (7, 8):  # LSTAT or FSTAT
        reply(105, request_id, attrs)
    elif kind == 3:  # OPEN
        reply(102, request_id, string(handle))
    elif kind == 4:  # CLOSE, success status with empty message and language
        reply(101, request_id, bytes(12))
    elif kind == 5:  # READ
        handle_length = struct.unpack_from(">I", packet, 5)[0]
        handle_end = 9 + handle_length
        assert packet[9:handle_end] == handle
        offset, count = struct.unpack(">QI", packet[handle_end:])
        assert 0 < count <= chunk_size and offset + count <= size
        read = (request_id, offset, count)
        if initial is not None:
            assert offset == len(initial) * chunk_size
            if mode == "reorder" and offset == 0:
                read = (request_id, offset, count // 3)
            initial.append(read)
            if len(initial) == window:
                if mode == "duplicate":
                    read_reply(*initial[-1])
                    read_reply(*initial[-1])
                else:
                    for request in reversed(initial):
                        read_reply(*request)
                initial = None
        elif mode == "latency":
            delayed.append((time.monotonic() + 0.100, read))
        elif mode != "stall":
            read_reply(*read)
    else:
        raise RuntimeError(f"unexpected SFTP request {kind}")
