"""Entrypoint invoked by the Rust daemon's supervisor.

Invoked as:
    python -m crawl4ai_adapter.server_main --host 127.0.0.1 --port 11235 --runtime-dir ...

Blocks until killed. Reports ready by binding the port; the supervisor
polls /health.
"""

from __future__ import annotations

import argparse
import logging
import os
import sys

import uvicorn


def main() -> int:
    parser = argparse.ArgumentParser(prog="python -m crawl4ai_adapter.server_main")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=11235)
    parser.add_argument("--runtime-dir", required=True)
    parser.add_argument("--log-level", default="info")
    args = parser.parse_args()

    os.environ["CRAWL4_AI_BASE_DIRECTORY"] = args.runtime_dir
    logging.basicConfig(
        level=args.log_level.upper(),
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
        stream=sys.stderr,
    )

    uvicorn.run(
        "crawl4ai_adapter.server:app",
        host=args.host,
        port=args.port,
        log_level=args.log_level,
        access_log=False,
        loop="asyncio",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
