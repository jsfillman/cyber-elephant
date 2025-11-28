# Epic 03 - Host Flow, Delivery, and Polish

Goal: Make it Jackbox-easy for the host to start a room, share a link/QR, run the session, and export results. Ensure local deploy is trivial.
Outcome: Host can spin up a game, display join info, monitor progress, and end with a sharable summary; app ships as a single backend binary serving static frontend or docker-compose.

## Stories
1) Host dashboard and controls
   - Host view showing lobby list, start button (locks submissions), and current phase/turn indicator.
   - Controls to kick a duplicate/idle player before start and to restart lobby after a finished game.
   - Clear status toasts when actions succeed/fail.

2) Join link and QR handoff
   - Generate canonical join URL for the active game; display short code and copy-to-clipboard.
   - Render QR code for big-screen display; responsive for projector/mobile.
   - Auto-refresh if game id changes; offline-friendly fallback instructions.

3) Results and export
   - End screen: list of `player -> gift -> link/hint` with ownership resolution.
   - Export options: copy-to-clipboard text and download JSON (or CSV) for host.
   - Guard against leaking host token; sanitize output.

4) Packaging and deployability
   - Backend serves static frontend build; single `cargo run` path.
   - docker-compose for backend + optional Postgres; documented env vars and ports.
   - Health endpoint and minimal logging format for ops visibility.

5) Observability and stability
   - Structured logging for actions/events; correlation by game id/player id.
   - Basic metrics counters (connections, actions, steals, errors) via simple middleware or stub.
   - Backpressure and reconnect handling validated with quick soak test script.

6) QA and playtest checklist
   - Scripted manual test plan for lobby, submissions, start, choose, steal, forced chains, and finish.
   - At least one headless/e2e smoke (e.g., Playwright or Vitest + jsdom) covering join + open + steal happy path.
   - Capture known limitations and fallback behaviors in README.
