# Agent Console (for Claude Code)

Inspect event logs, view file edits, search conversations, and analyze policy evaluations.

### Event Logs

Browse the full conversation history with timestamps. Filter by event type (me, context, assistant, system), drill into sub-agent sessions, and inspect raw JSON.

![Event Logs](docs/screenshots/1event-logs.png)

### File Edits

See every file change made during a session. Toggle between tree and log views, view side-by-side or unified diffs, and compare against git HEAD.

![File Edits](docs/screenshots/2view-edits.png)

### Boolean Search

Search across the entire session with AND/OR operators. Matching terms are highlighted in context snippets.

![Boolean Search](docs/screenshots/3boolean-search.png)

### Policy Viewer

Visualize [Cupcake](https://github.com/eqtylab/cupcake) policy evaluations with timing traces. See which policies matched, what decisions were made (Allow, Deny, Halt), and why.

![Policy Viewer](docs/screenshots/4cupcake-policy-viewer.png)

---

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) (v18+)
- [pnpm](https://pnpm.io/)

### Install Dependencies

```bash
pnpm install
```

### Development

```bash
pnpm tauri dev
```

### Build

```bash
pnpm tauri build
```

The built app will be in `src-tauri/target/release/bundle/`.
