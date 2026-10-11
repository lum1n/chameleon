# chameleon

One terminal UI for every agent harness. chameleon is a thin [ratatui](https://ratatui.rs) front
end over [ACC](https://github.com/lum1n/acc), the Agent Control Core, so it drives Claude Code,
Codex, Cursor Agent, Copilot CLI, pi and opencode through their own headless protocols, on this
machine and on any host you can `ssh` to, with nothing installed there.

Sessions are ACC's detached sessions (`~/.acc/s/<id>/` on the host): they survive chameleon
quitting, several clients can attach to one session, and the phone app
[Sessh](https://github.com/lum1n/sessh-swift) sees the same sessions.

```
┌ sessions ────────────┐┌ fix the tests  ~/proj · Claude Code ──────────────────────┐
│local               3 ││› fix the failing tests                                   │
│1 ⚠ fix the te… claude││                                                          │
│2 ⚙ bump deps   codex ││∴ Looking at the test output first.                       │
│3 · docs        pi    ││$ cargo test                                              │
│box                 1 ││  … 12 more lines (space)                                 │
│4 … refactor  opencode││  test result: FAILED. 3 passed; 1 failed                 │
│                      ││± src/lib.rs edit +4 −1  space: diff                      │
│                      ││                                                          │
│                      ││⚠ Allow Edit src/lib.rs?                                  │
│                      ││  1 Allow   2 Always allow   3 Deny   y approve · N deny  │
│                      │└──────────────────────────────────────────────────────────┘
│                      │┌ prompt ───────────────────────────────────────────────────┐
│                      ││                                                          │
└──────────────────────┘└──────────────────────────────────────────────────────────┘
 ⚠ approval Claude Code │ model opus  permission default │ ctx 42% $0.31 │ 5h 37%
```

## Build

chameleon depends on `acc-core` as a sibling checkout (`../acc`), until acc is published.

```sh
git clone https://github.com/lum1n/acc ../acc
cargo build --release
./target/release/chameleon
```

Rust 1.88 or newer. The optional Lua runtime needs `--features lua`.

## Use

| Key | Does |
|---|---|
| `n` / `ctrl-n` | start a session: host, harness, directory, model, permission, first prompt |
| `j` `k` `1`-`0` | pick a session; in the transcript, move between items |
| `enter` | attach to a session, or focus the prompt |
| typing | starts a prompt; `enter` sends, `alt-enter` adds a line |
| `/` | command palette: the harness's slash commands, chameleon actions, hooks |
| `y` / `N` | approve or deny the open request (`N` opens the card with the denial selected) |
| `o` | model, effort, mode, permission (as the harness reports them) |
| `space` | expand or collapse the item under the cursor (reasoning, output, diffs) |
| `d` | the latest file change as a diff |
| `ctrl-c` `c` `f` | interrupt, compact, fork |
| `x` / `D` | stop the harness / stop and delete the session |
| `r` | re-list sessions on every host |
| `?` | help, with your bindings and config problems |

Every key is remappable. Sessions are sorted by urgency: waiting for you first, then errors,
working, idle, done, exited.

## Configure

`~/.config/chameleon/config.toml` (or `--config PATH`). Everything is optional.

```toml
[general]
tick_ms = 250          # repaint cadence for spinners
list_refresh_s = 20    # how often hosts are re-listed
auto_attach = true     # follow every alive session found on a host
default_harness = "claude-code"

[[hosts]]
name = "local"         # this machine (implied when no hosts are listed)

[[hosts]]
name = "box"
ssh = "box"            # any ssh destination; ~/.ssh/config, agents and ControlMaster apply
ssh_args = ["-p", "2222"]
connect_timeout_s = 10

[keys]                 # action = "key" or ["key", "key"]; see ? for action names
quit = ["q", "ctrl-q"]
new_session = "ctrl-n"

[theme]                # named colours or #rrggbb
accent = "magenta"
permission = "red"

[[hooks]]              # run on events, for any session
name = "notify"
on = ["permission", "question", "error", "done"]
command = 'notify-send "$CHAM_HARNESS" "$CHAM_TITLE"'

[[hooks]]              # or on a key, for the selected session
name = "edit"
key = "ctrl-o"
command = 'cd "$CHAM_CWD" && $EDITOR'
suspend_tui = true     # hand the terminal to the command
```

Hooks run `sh -c command` on this machine with the session as JSON on stdin (`host`, `spec`,
`state`, `usage`, `quota`, `permissions`, `selected_item`) and as `CHAM_HOST`, `CHAM_SESSION_ID`,
`CHAM_HARNESS`, `CHAM_CWD`, `CHAM_STATE`, `CHAM_TITLE`, `CHAM_NATIVE_ID`, `CHAM_MODEL`,
`CHAM_EVENT`. Events: `permission`, `question`, `error`, `done`, `idle`, `exited`, `turn`,
`notice`. Replayed history never fires hooks.

## Lua

Built with `cargo build --features lua`, chameleon loads `~/.config/chameleon/init.lua` (or
`--lua PATH`). Hooks registered there run inside chameleon, with the same payload as shell hooks
as a table:

```lua
cham.hook {
  name = "ping", on = { "permission", "done" }, key = "ctrl-g",
  run = function(s)
    cham.notify(s.spec.harness .. ": " .. (s.spec.title or ""))      -- a toast
    local out, status, err = cham.exec(s.host, "git -C " .. s.spec.cwd .. " status --short")
    cham.log(out)                                                     -- to --log
  end,
}
```

## How it works

`crates/chameleon-app` is the app model, sans-IO like acc-core: it takes events (keys, script
output, log lines) and returns effects (run this script on that host, follow this file, run this
hook). Its tests replay acc's recorded harness sessions. `crates/chameleon` is the runtime: tokio,
crossterm and ratatui, a `Host` that runs scripts under `sh` locally or through the system `ssh`,
and one long-lived fan-in follower per host that tails every attached session's log.

```sh
cargo test --workspace
cargo run -p chameleon-app --example replay -- claude ~/.acc/s/<id>/out.ndjson   # render a log
CHAMELEON_LOG=/tmp/cham.log chameleon                                            # debug log
```
