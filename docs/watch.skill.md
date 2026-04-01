# browser39 watch — Real-Time Agent IPC

`browser39 watch` monitors a JSONL commands file for new appended lines, processes each command through the browser engine, and writes results to an output file. This enables real-time, file-based communication between any agent process and browser39.

## Usage

```bash
# Create the commands file (must exist before starting)
touch commands.jsonl

# Start watching
browser39 watch commands.jsonl --output results.jsonl
```

The watcher runs until it receives a `quit` action or the process is killed.

## How it works

1. On startup, processes any existing lines in the commands file
2. Watches the file's parent directory for modifications (FSEvents on macOS, inotify on Linux)
3. On each modification, reads only new complete lines appended since the last read
4. Dispatches each command through the browser engine
5. Writes results atomically (single `write()` + `fsync()` per line) to the output file
6. Rotates the output file at 10MB

Partial lines (no trailing newline) are held until the next write completes them.

## Appending commands

From another process (Python, shell, Node, etc.), append JSONL commands:

```bash
# Fetch a page
echo '{"id":"1","action":"fetch","v":1,"seq":1,"url":"https://example.com"}' >> commands.jsonl

# List links
echo '{"id":"2","action":"links","v":1,"seq":2}' >> commands.jsonl

# Follow a link by index
echo '{"id":"3","action":"fetch","v":1,"seq":3,"index":0}' >> commands.jsonl

# Navigate back
echo '{"id":"4","action":"back","v":1,"seq":4}' >> commands.jsonl

# Session info / heartbeat
echo '{"id":"5","action":"info","v":1,"seq":5}' >> commands.jsonl

# Shut down
echo '{"id":"6","action":"quit","v":1,"seq":6}' >> commands.jsonl
```

## Command envelope

Every command line must be valid JSON with these fields:

```json
{"id": "unique-id", "action": "fetch", "v": 1, "seq": 1, ...action fields}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique identifier, echoed in result |
| `action` | string | Action to perform |
| `v` | integer | Protocol version (always `1`) |
| `seq` | integer | Monotonically increasing sequence number |

The `seq` must be strictly greater than the previous command's `seq`. Out-of-order commands are rejected with `INVALID_COMMAND`.

## Result envelope

Each result is one JSON line in the output file:

```json
{"id": "unique-id", "ok": true, "seq": 1, ...result fields}
```

On error:
```json
{"id": "unique-id", "ok": false, "seq": 1, "code": "NO_PAGE", "error": "no page loaded"}
```

## Available actions

| Action | Description |
|--------|-------------|
| `fetch` | Load a URL, follow a link by index or text |
| `links` | List links on current page (no network request) |
| `dom_query` | Query DOM via CSS selector or JavaScript |
| `fill` | Fill form fields by CSS selector |
| `submit` | Submit a form by CSS selector |
| `cookies` | List cookies (optionally filtered by domain) |
| `set_cookie` | Set a cookie |
| `delete_cookie` | Delete a cookie by name and domain |
| `storage_get` | Get a localStorage value |
| `storage_set` | Set a localStorage value |
| `storage_delete` | Delete a localStorage key |
| `storage_list` | List localStorage entries |
| `storage_clear` | Clear localStorage for an origin |
| `back` | Navigate back in history |
| `forward` | Navigate forward in history |
| `info` | Session state and liveness heartbeat |
| `config` | Set run-level config (e.g. `step_delay`) |
| `quit` | Shut down the watcher |

See [jsonl-protocol.md](jsonl-protocol.md) for the full protocol specification including all action fields and options.

## Config action

Set run-level options at any point (typically first command):

```json
{"id":"cfg","action":"config","v":1,"seq":0,"step_delay":0.5}
```

`step_delay` adds a pause between commands — either a fixed number of seconds or a `[min, max]` range for random delays:

```json
{"step_delay": [0.5, 2.0]}
```

## Python example

```python
import json
import time

COMMANDS = "commands.jsonl"
RESULTS = "results.jsonl"

def send(action, seq, **fields):
    cmd = {"id": f"cmd-{seq}", "action": action, "v": 1, "seq": seq, **fields}
    with open(COMMANDS, "a") as f:
        f.write(json.dumps(cmd) + "\n")
        f.flush()

def read_results():
    with open(RESULTS) as f:
        return [json.loads(line) for line in f if line.strip()]

# Fetch a page
send("fetch", 1, url="https://example.com")
time.sleep(1)

# Get links
send("links", 2)
time.sleep(0.5)

# Done
send("quit", 3)
time.sleep(0.5)

for r in read_results():
    print(f"seq={r['seq']} ok={r['ok']}")
```

## Differences from batch mode

| | `browser39 batch` | `browser39 watch` |
|---|---|---|
| Reads | All lines at startup | New lines continuously |
| Exits | After processing all lines | On `quit` action |
| File must exist | Yes | Yes |
| Use case | Script-driven, one-shot | Long-running agent IPC |
