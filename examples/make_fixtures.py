#!/usr/bin/env python3
"""Regenerates the synthetic example casts.

No real programs are recorded here: fixtures are deterministic scripts so the
repo's demos re-render byte-identically. Currently produces
`agent-session.cast` (+ input events), a believable ~42s agentic-TUI session
with a typed prompt, a long thinking pause, a streamed diff, and a test run —
the raw material for the before/after launch demo (SPEC §13).
"""

import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))

GREEN = "\x1b[32m"
RED = "\x1b[31m"
CYAN = "\x1b[36m"
DIM = "\x1b[2m"
BOLD = "\x1b[1m"
MAGENTA = "\x1b[35m"
RESET = "\x1b[0m"

SPINNER = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"


def agent_session():
    events = []
    t = 0.0

    def out(data, dt=0.0):
        nonlocal t
        t += dt
        events.append((round(t, 4), "o", data))

    def key(ch, dt):
        nonlocal t
        t += dt
        events.append((round(t, 4), "i", ch))
        events.append((round(t, 4), "o", ch))

    out(f"{DIM}$ {RESET}opencode\r\n", 0.4)
    out(f"{MAGENTA}◆{RESET} {BOLD}opencode{RESET} {DIM}v0.9 — gpt-5.2-codex{RESET}\r\n\r\n", 0.7)
    out(f"{CYAN}>{RESET} ", 0.4)

    prompt = "refactor auth to use signed session tokens"
    for i, ch in enumerate(prompt):
        key(ch, 0.11 if ch != " " else 0.16)
    key("\r", 0.5)
    out("\r\n\r\n", 0.0)

    # The thinking pause — the region every edited demo speed-ramps.
    out(f"{DIM}● thinking", 0.3)
    spin_start = t
    for i in range(150):  # ~19s of spinner at 8Hz
        out(f"\r{DIM}● thinking {SPINNER[i % len(SPINNER)]} {RESET}", 0.126)
    out(f"\r{GREEN}✓{RESET} planned the change {DIM}(19.1s){RESET}      \r\n", 0.3)

    out(f"{GREEN}✓{RESET} read {BOLD}src/auth.rs{RESET} {DIM}(214 lines){RESET}\r\n", 0.9)
    out(f"{GREEN}✓{RESET} read {BOLD}src/session.rs{RESET} {DIM}(87 lines){RESET}\r\n", 0.6)
    out(f"{MAGENTA}●{RESET} editing {BOLD}src/auth.rs{RESET}\r\n\r\n", 0.8)

    diff = [
        (DIM, "  @@ -41,9 +41,11 @@ impl AuthService {"),
        (RED, "  -    pub fn login(&self, user: &str, pw: &str) -> bool {"),
        (RED, "  -        self.sessions.insert(user.into());"),
        (RED, "  -        self.check(user, pw)"),
        (GREEN, "  +    pub fn login(&self, user: &str, pw: &str) -> Result<Token> {"),
        (GREEN, "  +        self.check(user, pw)?;"),
        (GREEN, "  +        let token = Token::signed(user, self.key)?;"),
        (GREEN, "  +        self.sessions.insert(token.id());"),
        (GREEN, "  +        Ok(token)"),
        (RED, "  -    }"),
        (GREEN, "  +    }"),
        (DIM, "  @@ -63,4 +65,12 @@ impl AuthService {"),
        (GREEN, "  +    pub fn verify(&self, token: &Token) -> bool {"),
        (GREEN, "  +        token.valid(self.key) && self.sessions.contains(&token.id())"),
        (GREEN, "  +    }"),
    ]
    for color, line in diff:
        out(f"{color}{line}{RESET}\r\n", 0.28)

    out(f"\r\n{MAGENTA}●{RESET} running {BOLD}cargo test{RESET}\r\n", 1.1)
    out(f"{DIM}   Compiling authd v0.4.1{RESET}\r\n", 1.7)
    out(f"{DIM}    Finished test profile in 2.84s{RESET}\r\n", 2.9)
    out("     Running unittests src/lib.rs\r\n", 0.4)
    out("\r\nrunning 24 tests\r\n", 0.5)
    for i in range(3):
        out("........" if i < 2 else "........\r\n", 0.55)
    out(f"\r\ntest result: {GREEN}ok{RESET}. 24 passed; 0 failed\r\n", 0.4)
    out(f"\r\n{GREEN}✓ done{RESET} — auth now issues signed tokens {DIM}(41.8s){RESET}\r\n", 0.9)

    header = {
        "version": 2,
        "width": 88,
        "height": 24,
        "title": "synthetic agent session (examples/make_fixtures.py)",
        "env": {"TERM": "xterm-256color"},
    }
    cast_path = os.path.join(HERE, "agent-session.cast")
    with open(cast_path, "w") as f:
        f.write(json.dumps(header) + "\n")
        for ev in events:
            f.write(json.dumps(list(ev)) + "\n")

    meta = {
        "version": 1,
        "input_events": [
            {"t": ev[0], "kind": "key", "value": ev[2]}
            for ev in events
            if ev[1] == "i"
        ],
        "term_env": {"TERM": "xterm-256color"},
        "cols": 88,
        "rows": 24,
    }
    with open(cast_path + ".reelmeta", "w") as f:
        json.dump(meta, f, indent=2)
    print(f"wrote {cast_path} ({t:.1f}s, {len(events)} events)")


if __name__ == "__main__":
    agent_session()
