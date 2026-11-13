#!/usr/bin/env python3
"""Exercise an already-built image, including read-only/nonroot AUTH and SIGTERM."""
import argparse
import json
import secrets
import subprocess
import tempfile
import time
from pathlib import Path

from resp import Client
from differential import REDIS


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True).strip()


def smoke(image, authenticated):
    with tempfile.TemporaryDirectory() as directory:
        password = secrets.token_hex(24)
        path = Path(directory) / "password"
        path.write_text(password + "\n")
        path.chmod(0o444)
        options = []
        if authenticated:
            options = ["--mount", f"type=bind,src={path},dst=/run/remik-password,readonly", "--env", "REMIK_PASSWORD_FILE=/run/remik-password"]
        container = docker("run", "-d", "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--memory=256m", "--cpus=1", "--pids-limit=128", "--publish", "127.0.0.1::6379", *options, image)
        client = None
        try:
            port = docker("port", container, "6379/tcp").rsplit(":", 1)[1]
            for _ in range(60):
                state = json.loads(docker("inspect", container))[0]["State"]
                if not state["Running"]:
                    raise RuntimeError("container exited before readiness")
                if state.get("Health", {}).get("Status") == "healthy":
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError("container never became healthy")
            assert docker("exec", container, "id", "-u") == "10001"
            docker("exec", container, "remik", "--version")
            docker("exec", container, "remik", "healthcheck", "--bind", "0.0.0.0:6379")
            client = Client("127.0.0.1", port)
            if authenticated:
                denied = client.command("PING")
                assert isinstance(denied, tuple) and denied[0] == b"-", "unauthenticated request accepted"
                assert client.command("AUTH", password) == (b"+", b"OK")
            assert client.command("PING") == (b"+", b"PONG")
            assert client.command("SET", "smoke", b"a\x00b") == (b"+", b"OK")
            assert client.command("GET", "smoke") == b"a\x00b"
            auth = ["--env", f"REDISCLI_AUTH={password}"] if authenticated else []
            assert docker("run", "--rm", "--read-only", "--cap-drop=ALL", "--network", f"container:{container}", "--entrypoint", "redis-cli", *auth, REDIS, "-2", "--raw", "PING") == "PONG", "redis-cli compatibility failed"
            # Keep this client open: shutdown must not hang on an idle reader.
            started = time.monotonic()
            docker("stop", "--time", "10", container)
            assert docker("wait", container) == "0", "SIGTERM was not a clean exit"
            assert time.monotonic() - started < 10, "shutdown exceeded deadline"
        finally:
            if client:
                client.close()
            docker("rm", "-f", container)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", nargs="?", default="remik:ci")
    args = parser.parse_args()
    smoke(args.image, False)
    smoke(args.image, True)
    print("Docker smoke passed (plain + AUTH, healthcheck, UID, read-only, SIGTERM)")
