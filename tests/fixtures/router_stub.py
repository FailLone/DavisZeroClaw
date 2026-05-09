"""Test stub: prints a fixed final-line JSON status, then exits."""
import json
import sys

print("stub: pretending to talk to router")
sys.stdout.write(json.dumps({
    "status": "ok",
    "action": "none",
    "dhcp_was_enabled": False,
    "duration_ms": 42,
}) + "\n")
sys.stdout.flush()
sys.exit(0)
