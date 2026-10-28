"""Read-only SFTP v3 subset for SSH shell links (Python 3.6+, Unix).

The program is fixed source, passed as one quoted SSH command. All requested
paths arrive over the binary protocol; none are evaluated as shell/Python code.
No files are installed on the remote machine. EOF closes every open resource.
"""
import errno
import os
import stat
import struct
import sys

LIMIT = 1024 * 1024
handles = {}
serial = 0


def u32(value):
    return struct.pack(">I", value)


def string(value):
    if len(value) > 65536:
        raise ValueError("string exceeds 64 KiB")
    return u32(len(value)) + value


def exact(count):
    result = bytearray()
    while len(result) < count:
        chunk = sys.stdin.buffer.read(count - len(result))
        if not chunk:
            raise EOFError()
        result.extend(chunk)
    return bytes(result)


def send(kind, payload):
    packet = bytes([kind]) + payload
    if len(packet) > LIMIT:
        raise ValueError("packet exceeds 1 MiB")
    data = memoryview(u32(len(packet)) + packet)
    while data:
        count = sys.stdout.buffer.write(data)
        if not count:
            raise BrokenPipeError()
        data = data[count:]
    sys.stdout.buffer.flush()


class Reader:
    def __init__(self, data):
        self.data = data

    def take(self, count):
        if count > len(self.data):
            raise ValueError("truncated request")
        value, self.data = self.data[:count], self.data[count:]
        return value

    def number(self):
        return struct.unpack(">I", self.take(4))[0]

    def string(self):
        count = self.number()
        if count > 65536:
            raise ValueError("string exceeds 64 KiB")
        return self.take(count)

    def path(self):
        path = self.string()
        if not path or b"\0" in path:
            raise ValueError("empty path or embedded NUL")
        return path

    def finish(self):
        if self.data:
            raise ValueError("trailing request bytes")


def attributes(info):
    atime = max(0, min(0xffffffff, int(info.st_atime)))
    mtime = max(0, min(0xffffffff, int(info.st_mtime)))
    token = ("%d:%d:%d:%d" % (info.st_dev, info.st_ino, info.st_ctime_ns,
                              info.st_mtime_ns)).encode("ascii")
    return (u32(0x8000000f) + struct.pack(">QIIIII", info.st_size,
            info.st_uid, info.st_gid, info.st_mode, atime, mtime) + u32(1)
            + string(b"rc-identity@rust-commander") + string(token))


def status(request, code, message=b""):
    send(101, request + u32(code) + string(message[:4096]) + string(b""))


def new_handle(kind, value):
    global serial
    serial += 1
    handle = struct.pack(">Q", serial)
    handles[handle] = (kind, value)
    return handle


def get_handle(reader, expected):
    kind, value = handles[reader.string()]
    if kind != expected:
        raise ValueError("wrong handle type")
    return value


def close(kind, value):
    if kind == "file":
        os.close(value)
    else:
        value.close()


def dispatch(kind, request, reader):
    if kind in (3, 11) and len(handles) >= 32:
        raise ValueError("too many open handles")
    if kind == 16:  # REALPATH
        path = reader.path()
        reader.finish()
        path = os.path.realpath(path)
        info = os.lstat(path)
        send(104, request + u32(1) + string(path) + string(b"") + attributes(info))
    elif kind == 7:  # LSTAT
        path = reader.path()
        reader.finish()
        send(105, request + attributes(os.lstat(path)))
    elif kind == 11:  # OPENDIR
        path = reader.path()
        reader.finish()
        send(102, request + string(new_handle("directory", os.scandir(path))))
    elif kind == 12:  # READDIR
        directory = get_handle(reader, "directory")
        reader.finish()
        entries = []
        size = 0
        for _ in range(16):
            entry = next(directory, None)
            if entry is None:
                break
            encoded = string(entry.name) + string(b"") + attributes(entry.stat(follow_symlinks=False))
            size += len(encoded)
            if size > LIMIT - 9:
                raise ValueError("directory batch too large")
            entries.append(encoded)
        if entries:
            send(104, request + u32(len(entries)) + b"".join(entries))
        else:
            status(request, 1)
    elif kind == 3:  # OPEN: read-only regular files, no symlink or FIFO blocking
        path = reader.path()
        flags, attrs = reader.number(), reader.number()
        reader.finish()
        if flags != 1 or attrs != 0:
            status(request, 3, b"shell links are read-only")
            return
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            if not stat.S_ISREG(os.fstat(fd).st_mode):
                raise ValueError("regular files only")
        except BaseException:
            os.close(fd)
            raise
        send(102, request + string(new_handle("file", fd)))
    elif kind == 8:  # FSTAT
        fd = get_handle(reader, "file")
        reader.finish()
        send(105, request + attributes(os.fstat(fd)))
    elif kind == 5:  # READ
        fd = get_handle(reader, "file")
        offset = struct.unpack(">Q", reader.take(8))[0]
        count = reader.number()
        reader.finish()
        if not 0 < count <= 32768:
            raise ValueError("invalid read size")
        data = os.pread(fd, count, offset)
        if data:
            send(103, request + string(data))
        else:
            status(request, 1)
    elif kind == 4:  # CLOSE
        handle = reader.string()
        reader.finish()
        close(*handles.pop(handle))
        status(request, 0)
    else:
        status(request, 8, b"shell links support read-only operations")


def main():
    initialized = False
    while True:
        length = struct.unpack(">I", exact(4))[0]
        if not 1 <= length <= LIMIT:
            raise ValueError("invalid packet length")
        packet = Reader(exact(length))
        kind = packet.take(1)[0]
        if not initialized:
            if kind != 1 or packet.number() < 3:
                raise ValueError("SFTP version 3 handshake required")
            packet.finish()
            send(2, u32(3))
            initialized = True
            continue
        request = packet.take(4)
        try:
            dispatch(kind, request, packet)
        except OSError as error:
            code = 2 if error.errno == errno.ENOENT else 3 if error.errno in (errno.EACCES, errno.EPERM) else 4
            status(request, code, str(error).encode("utf-8", "backslashreplace"))
        except (ValueError, KeyError, OverflowError) as error:
            status(request, 5, str(error).encode("utf-8", "backslashreplace"))


try:
    main()
except (EOFError, BrokenPipeError):
    pass
finally:
    for resource in handles.values():
        close(*resource)
