"""Durable writes and canonical identities shared by trusted runner journals."""
import json
import os
import uuid

class Rejected(Exception):
    def __init__(self, code, message):
        self.code, self.message = code, message


def save(path, value):
    temporary = path.with_suffix(".new")
    with temporary.open("w") as f:
        json.dump(value, f, separators=(",", ":"), sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(temporary, path)
    sync_directory(path.parent)


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def identity(value):
    parsed = str(uuid.UUID(value))
    if value != parsed:
        raise Rejected(400, "canonical UUID required")
    return parsed
