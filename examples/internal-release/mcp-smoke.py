#!/usr/bin/env python3
"""Initialize pc-mcp, list its tools, and call the read-only revision tool."""

import json
import subprocess
import sys

server, plan_dir = sys.argv[1:]
process = subprocess.Popen(
    [server], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
)

def send(message):
    process.stdin.write(json.dumps(message) + "\n")
    process.stdin.flush()

def receive():
    line = process.stdout.readline()
    if not line:
        raise RuntimeError("pc-mcp closed stdout before replying")
    return json.loads(line)

send({
    "jsonrpc": "2.0", "id": 1, "method": "initialize",
    "params": {
        "protocolVersion": "2025-03-26", "capabilities": {},
        "clientInfo": {"name": "pc-internal-release-example", "version": "1"},
    },
})
initialized = receive()
if "result" not in initialized:
    raise RuntimeError(f"MCP initialize failed: {initialized}")
send({"jsonrpc": "2.0", "method": "notifications/initialized"})
send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
listed = receive()
names = {tool["name"] for tool in listed["result"]["tools"]}
expected = {"check", "compile", "frontier", "context", "impact", "diff",
            "revision", "propose", "accept", "reject", "record"}
if not expected <= names:
    raise RuntimeError(f"MCP tools missing: {sorted(expected - names)}")
send({
    "jsonrpc": "2.0", "id": 3, "method": "tools/call",
    "params": {"name": "revision", "arguments": {"path": plan_dir}},
})
called = receive()
if "result" not in called or called["result"].get("isError"):
    raise RuntimeError(f"MCP revision call failed: {called}")
print(f"MCP tools listed ({len(names)}); revision returned for {plan_dir}")
process.stdin.close()
if process.wait(timeout=5) != 0:
    raise RuntimeError("pc-mcp exited unsuccessfully")
