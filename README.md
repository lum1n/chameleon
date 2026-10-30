# chameleon

One terminal UI for every agent harness. chameleon is a thin [ratatui](https://ratatui.rs) front
end over [ACC](https://github.com/lum1n/acc), the Agent Control Core, so it drives Claude Code,
Codex, Cursor Agent, Copilot CLI, pi and opencode through their own headless protocols, on this
machine and on any host you can `ssh` to, with nothing installed there.

Sessions are ACC's detached sessions (`~/.acc/s/<id>/` on the host): they survive chameleon
quitting, several clients can attach to one session, and the phone app
[Sessh](https://github.com/lum1n/sessh-swift) sees the same sessions.

```
  › fix the tests  ~/proj · Claude Code · opus                 ⚠1 ⠋2 ·1
────────────────────────┬───────────────────────────────────────────────────────────
 local ─────────────── 3│ ▎ fix the failing tests
▌1 ⚠ fix the te… claude │
 2 ⚙ bump deps    codex │ ∴ Looking at the test output first.
 3 · docs            pi │ $ cargo test
                        │   … 12 more lines (space)
 box ───────────────── 1│   test result: FAILED. 3 passed; 1 failed
 4 ⠋ refactor  opencode │ ± src/lib.rs edit +4 −1  space: diff
                        │
                        │ ⚠ Allow Edit src/lib.rs?
                        │   1 Allow   2 Always allow   3 Deny   y approve · N deny
                        ├───────────────────────────────────────────────────────────
                        │ ❯ type to prompt · / commands
 ⚠ approval Claude Code │ model opus · permission default │ ctx ▰▰▰▱▱▱ 42% · $0.31
```

`b` folds the session list away so the transcript has the whole terminal; the header keeps a
tally of every session's state (`⚠1 ⠋2 ·1`) so nothing gets lost while it's hidden.

## Build

chameleon depends on `acc-core` from [GitHub](https://github.com/lum1n/acc), which cargo fetches
itself, until acc is published.

```sh
cargo build --release
./target/release/chameleon
```

Rust 1.88 or newer. The optional Lua runtime needs `--features lua`.

## Use

| Key | Does |
|---|---|
| `n` / `ctrl-n` | start a session: host, harness, directory, worktree, sandbox, model, permission, first prompt. The directory completes from the host: `tab` completes, `↓` `↑` pick, `enter` takes. With a worktree, the session works in a git worktree of its own (`~/.acc/w/<id>`, branch `acc/<id>`), so several agents can share a repository. With a sandbox, acc confines the harness (bubblewrap on Linux, Seatbelt on macOS, chosen on the host): it writes only the directory and its repository (or nothing there, read only), temporary directories, caches and its own state, and can't read `~/.ssh` keys, `~/.aws` and other credentials. The wizard says beforehand what the host can do; one that can't still runs the harness, unconfined, and the header says so. A harness older than acc was tested with is flagged |
| `j` `k` `1`-`0` | pick a session; in the transcript, move between items |
| `ctrl-j` `ctrl-k` / `ctrl-h` `ctrl-l` | in every list, picker and form (the palette and launcher too): down / up and change a choice, even while typing. Plain `hjkl` and `g` `G` where nothing is typed |
| `b` / `ctrl-b` | show or hide the session list (`h` brings it back too) |
| `T` | colour theme: previews as you move, `enter` keeps it across restarts |
| `B` | transcript blocks: coloured bars, boxed messages, or plain |
| `enter` | attach to a session, or focus the prompt |
| typing | starts a prompt; `enter` sends, `alt-enter` adds a line |
| `ctrl-v` / drop a file | attach an image to the prompt: `ctrl-v` takes the clipboard's (`wl-paste`, `xclip` or AppleScript, on the machine chameleon runs on), and pasting or dropping image files on the terminal attaches them instead of their paths. PNG, JPEG, GIF and WebP, up to 5 MB each and 10 MB a prompt. They travel inside the prompt, so a harness on a host over `ssh` gets them with nothing copied there. `backspace` at the start of the prompt drops the last one. An ACP agent that doesn't take images refuses the prompt, which stays in the composer |
| `/` | command palette: the harness's slash commands, chameleon actions, hooks. Commands that take a value (`/model`, `/mode`) open a picker, dialogs (`/mcp`, `/status`) open a panel with their actions, and TUI-only ones (`/doctor`) hand the terminal to the harness |
| `y` / `N` | approve once or deny the open request (`N` opens the card with the denial selected) |
| `A` | auto-approve: for harnesses without a mode that skips approvals. While it's on (`auto` in the header), chameleon allows every approval the harness asks for, once, except what looks dangerous: deleting (`rm`, `find -delete`, `git clean`, `git reset --hard`, ...), `sudo` and other escalation, `kill`, `chmod`, `git push`, publishing, a script piped into a shell, files that may hold secrets (`.env`, `.ssh`, keys), system files, dotfiles outside the project (`~/.config`, `~/.bashrc`) and edits outside the project. Those, questions, and requests that don't say what they run, are left to you, and a toast says why. Off at every start unless the config says otherwise |
| `o` | model, effort, mode, permission (as the harness reports them) |
| `space` | expand or collapse the item under the cursor (reasoning, output, diffs) |
| mouse | in the transcript, click a URL to open it in your browser; drag to select text, which is copied when you let go (with OSC 52, so it reaches your clipboard over SSH too, and with `wl-copy`, `xclip` or `pbcopy` when chameleon runs locally). Gutters and borders stay out of what's copied. A click elsewhere picks a session or focuses a pane, and the wheel scrolls. Your terminal's own selection still works with `shift` held |
| `enter` / `esc` | on a subagent step, open its conversation; `esc` goes back to where you were |
| `[` `]` / `a` | step through main and every subagent run / pick one from a list, running ones spinning |
| `d` | the diff viewer: the repository's uncommitted changes on the session's host (untracked files too; for a worktree session, everything it changed there, commits included), or with `s` every edit the agent made this session, merged per file, and on Codex, which diffs its turns, what the latest turn changed (shell commands' changes too). `n`/`p` file, `]`/`[` hunk, `r` reload. On an edit step it opens on that file |
| `M` / palette | worktree sessions: commit the changes and squash-merge them into the branch checked out in the repository (`M` in the diff viewer too). A conflict merges nothing. "discard the session's worktree" (or `X` in the diff viewer) stops the harness and deletes the worktree and its branch |
| `H` | hand the conversation to another harness: it starts on the same directory (and worktree), primed with the conversation so far; a prompt you have typed goes as the next message. The first session keeps running |
| `U` | usage: what sessions spent, per account (with its quota windows) and per local day, as the harnesses reported it. Kept across restarts in `~/.local/state/chameleon/usage.json` |
| `L` | skills and MCP servers on the selected session's host (or the first host), one column per harness. Skills live once in `~/.agents/skills` and are symlinked into each harness's own directory: `space` links or unlinks the skill for the harness under the cursor (`h`/`l`), `a`/`u` for every harness, `A` adopts a skill that lives only in one harness's directory, `X X` deletes it. On the `mcp` tab (`tab`), `space` copies a server to the harness or removes it there, `e` switches it off and on where the harness can keep it off, `n` adds one (`name command args…` or `name https://url`), `X X` deletes it everywhere. Config files are edited one entry at a time and backed up to `~/.acc/backups/` before each write. `p` switches to the selected session's project |
| `ctrl-c` `c` `f` | interrupt, compact, fork |
| `x` / `D` | stop the harness / stop and delete the session |
| `r` | re-list sessions on every host |
| `?` | help, with your bindings and config problems |

While the agent keeps a to-do list (Claude's `TodoWrite`, Codex's plan, an ACP agent's plan) and
has work left on it, the list is pinned under the transcript. Sessions take the name their harness
gives them, and a fresh Claude session is named from its first prompt, as Claude's
own TUI does.

Every key is remappable. Sessions are listed oldest first and keep their place, so a
session's jump key (`1`–`0`) stays the same until a session above it is deleted.

## Configure

`~/.config/chameleon/config.toml` (or `--config PATH`). Everything is optional.

```toml
[general]
tick_ms = 250          # repaint cadence for spinners
list_refresh_s = 20    # how often hosts are re-listed
auto_attach = true     # follow every alive session found on a host
show_sidebar = true    # start with the session list open (b toggles it)
sidebar_width = 34
default_harness = "claude-code"
blocks = "bars"         # how messages are set apart: bars, cards, plain (B cycles)
nerd_font = true       # Nerd Font glyphs (the chameleon in the header); false: emoji and name

[[hosts]]
name = "local"         # this machine (implied when no hosts are listed)

[[hosts]]              # this machine is always listed first, even with hosts configured
name = "box"
ssh = "box"            # any ssh destination; ~/.ssh/config, agents and ControlMaster apply
ssh_args = ["-p", "2222"]
connect_timeout_s = 10

[keys]                 # action = "key" or ["key", "key"]; see ? for action names
quit = ["q", "ctrl-q"]
new_session = "ctrl-n"

[theme]
name = "tokyo-night"   # any Sessh or Lapp theme; unset: the terminal's own colours
background = true      # false keeps the terminal's background (transparent terminals)
accent = "#ff00ff"     # override any role: accent permission error tool thinking done
                       # muted user agent add remove border (named colours or #rrggbb)

[[hooks]]              # run on events, for any session
name = "notify"
on = ["permission", "question", "error", "done"]
command = 'notify-send "$CHAM_HARNESS" "$CHAM_TITLE"'

[[hooks]]              # or on a key, for the selected session
name = "edit"
key = "ctrl-o"
command = 'cd "$CHAM_CWD" && $EDITOR'
suspend_tui = true     # hand the terminal to the command

[[policy]]             # answer approvals without asking, on every harness
decision = "allow"
tool = "shell"         # the harness's tool name, or shell, edit, read, fetch, mcp
command = "git status" # a command's start, whole words; never matches chained commands

[[policy]]
decision = "deny"      # deny wins over allow; what no rule matches is left to you
path = "**/.env"       # files the tool acts on, relative to the session's directory

[auto_approve]         # A toggles it; these only add to what it leaves to you
enabled = false        # start with it on
ask_commands = ["make deploy"]  # a command's start, whole words
ask_paths = ["migrations/**"]   # relative to the session's directory, or ~/...

[watch]                # on the host of every session started here, even with chameleon
url = "https://ntfy.sh/my-topic"  # closed: post each approval waiting, turn done, error
# command = "..."      # and exit, or run a command there (event JSON on stdin)

[sandbox]              # sandbox new sessions by default (the wizard can turn it off)
preset = "workspace"   # or "read_only": the directory isn't writable either
write = ["~/scratch"]  # more writable paths
hide = ["~/secrets"]   # more unreadable paths
# show_credentials = true
```

The policy answers for every session chameleon attaches to, and says so in a toast. Set it in
one client only (not Sessh as well), or both answer the same request. The policy goes first:
auto-approval only sees what no rule answered, so a `deny` rule still denies while it's on.
Like the policy, auto-approval answers every session this chameleon is attached to.

## Themes

chameleon ships the same themes as [Sessh](https://github.com/lum1n/sessh-swift) and Lapp:
Phosphor, Kanagawa, Catppuccin, Gruvbox, Dracula, Nord, Tokyo Night, One, Solarized, Rosé Pine,
Everforest, Ayu, Flexoki, Vesper, GitHub, Oxocarbon, Poimandres, Night Owl, Material, Paper, Ink,
Forest, Nightfox and the rest, dark and light. Lapp's ids (`tokyonight`, `onedark`) work too.

A theme copied out of Sessh's theme editor is a JSON file chameleon reads: put it (or an array
of them) in `~/.config/chameleon/themes/*.json` and it shows up first in the picker. The theme
picked with `T` is remembered in `~/.local/state/chameleon/theme` and wins over the config.

chameleon runs `ssh` non-interactively (`BatchMode=yes`), so it can't type passwords or answer
host-key prompts: `ssh -o BatchMode=yes box echo ok` must print `ok` from a plain shell first
(`ssh-copy-id box`, an agent with your key, and a `ControlMaster auto` entry in `~/.ssh/config`
all help). When a host fails, the reason is shown under its name in the sidebar.

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
