#!/usr/bin/env python3
"""Compare supported string commands using disposable local Remik and Redis."""
import argparse
import random
import socket
import subprocess
import time

from resp import Client

REDIS = "redis:8.10.3@sha256:1f88291b4c57bdf249f02eb085a8669889b2d2954e91d4de5118a5da6b78dd53"


def run(binary):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    container = subprocess.check_output(["docker", "run", "-d", "--read-only", "--cap-drop=ALL", "--user", "999:999", "--workdir", "/tmp", "--publish", "127.0.0.1::6379", REDIS, "redis-server", "--bind", "0.0.0.0", "--protected-mode", "no", "--save", "", "--appendonly", "no"], text=True).strip()
    process = None
    clients = []
    try:
        process = subprocess.Popen([binary, "--bind", f"127.0.0.1:{port}"])
        redis_port = subprocess.check_output(["docker", "port", container, "6379/tcp"], text=True).strip().rsplit(":", 1)[1]
        for number in (port, redis_port):
            for _ in range(60):
                client = None
                try:
                    client = Client("127.0.0.1", number)
                    if client.command("PING") == (b"+", b"PONG"):
                        clients.append(client)
                        break
                except (OSError, ValueError):
                    # A starting reference container can accept then close a probe.
                    pass
                if client is not None:
                    client.close()
                if process.poll() is not None:
                    raise RuntimeError("Remik exited before readiness")
                if subprocess.check_output(["docker", "inspect", "--format", "{{.State.Running}}", container], text=True).strip() != "true":
                    raise RuntimeError(subprocess.check_output(["docker", "logs", container], text=True))
                else:
                    time.sleep(0.1)
            else:
                raise RuntimeError("server never became ready")
        commands = [
            ("PING",), ("PING", "hello"), ("ECHO", b"\x00\xff\r\n"),
            ("GET", "missing"), ("SET", "a", "1"), ("GET", "a"),
            ("SET", "a", "2", "NX"), ("SET", "a", "3", "XX"),
            ("MSET", "b", "hello", "c", "world"), ("MGET", "a", "b", "absent"),
            ("INCR", "a"), ("DECR", "a"), ("APPEND", "b", "!"), ("STRLEN", "b"),
            ("EXISTS", "a", "b", "missing"), ("DEL", "a", "b", "missing"),
            ("GET", "a"), ("SET", "binary", b"\x00\xff"), ("GET", "binary"),
            ("SET", b"\x00\xffkey", b""), ("GET", b"\x00\xffkey"),
            ("SET", "conditional", "old", "GET"),
            ("SET", "conditional", "skip", "NX", "GET"),
            ("SET", "conditional", "new", "XX", "GET"),
            ("MSET", "duplicate", "one", "duplicate", "two"), ("GET", "duplicate"),
            ("EXISTS", "duplicate", "duplicate"), ("DEL", "duplicate", "duplicate"),
            ("SET", "counter", "9223372036854775807"), ("INCR", "counter"),
            ("GET", "counter"), ("SET", "counter", "-9223372036854775808"),
            ("DECR", "counter"), ("DECRBY", "counter", "-9223372036854775808"),
            ("SET", "counter", "01"), ("INCR", "counter"), ("GET", "counter"),
            ("SET", "expiry", "value", "PX", "60000"),
            ("EXPIRE", "expiry", "60", "NX"), ("EXPIRE", "expiry", "120", "XX", "GT"),
            ("EXPIRE", "expiry", "60", "XX", "LT"),
            ("SET", "expiry", "retained", "KEEPTTL"),
            ("PERSIST", "expiry"), ("PTTL", "expiry"), ("PERSIST", "expiry"),
            ("PEXPIRE", "expiry", "0"), ("TTL", "expiry"),
            ("SET", "atomic", "before"), ("SET", "atomic", "after", "NX", "XX"),
            ("GET", "atomic"), ("MSET", "atomic", "bad", "incomplete"), ("GET", "atomic"),
            ("TYPE", "atomic"), ("TYPE", "does-not-exist"),
        ]
        rng = random.Random(42)
        for _ in range(300):
            key = f"random:{rng.randrange(16)}"
            operation = rng.choice(("SET", "GET", "DEL", "INCRBY", "MGET", "EXISTS"))
            args = (str(rng.randrange(-100, 100)),) if operation in ("SET", "INCRBY") else ()
            commands.append((operation, key, *args))
        for command in commands:
            actual = [client.command(*command) for client in clients]
            # Error wording is not part of Remik's compatibility promise.
            actual = [(value[0], value[1].split(b" ", 1)[0]) if isinstance(value, tuple) and value[0] == b"-" else value for value in actual]
            assert actual[0] == actual[1], f"differential mismatch for {command[0]}: {actual!r}"
        print(f"Differential passed: {len(commands)} commands against Redis 8.10.3")
    finally:
        try:
            for client in clients:
                client.close()
        finally:
            try:
                if process:
                    process.terminate()
                    try:
                        assert process.wait(timeout=10) == 0
                    finally:
                        if process.poll() is None:
                            process.kill()
                            process.wait()
            finally:
                subprocess.run(["docker", "rm", "-f", "--volumes", container], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/debug/remik")
    run(parser.parse_args().binary)
