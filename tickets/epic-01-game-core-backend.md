# Epic 01 - Game Core and Backend

Goal: Stand up the authoritative game engine and Axum backend that enforces all rules, exposes REST for setup, and WebSockets for real-time play.
Outcome: Deterministic state machine with tests; lobby/join/start flows; stable WebSocket hub that keeps all clients in sync.

## Stories
1) Game core crate and state reducer
   - To Do: set up Rust workspace + `game-core` crate; define core types (`Game`, `Player`, `Gift`, `Action`, `GameEvent`, `GamePhase`, `GameError`) with serde; pure reducer `apply_action`; helper to compute next active player/turn; error mapping.
   - TDD (Red → Green):
     - Red: tests for open gift happy path (emits gift_opened + turn_changed), steal happy path (gift_stolen + turn_changed), steal limit rejection, no immediate steal-back rejection, forced steal chain continues until new gift opened, invalid phase/turn errors.
     - Green: implement reducer logic until all tests pass; ensure deterministic outputs.
   - Acceptance: All rule paths tested; reducer is pure/deterministic and side-effect free; cargo test passes in game-core.

2) Lobby/session lifecycle REST
   - To Do: Axum skeleton + router; POST /game (create id + host token), POST /game/{id}/join (name -> player id), GET /game/{id} (lobby state); in-memory store keyed by game id; host token check; shared DTOs and error mapping; ID generation util.
   - TDD (Red → Green):
     - Red: handler tests for create returns game_id + host_token; join succeeds with unique names; duplicate name returns 409; lobby lists players in join order; unknown game returns 404.
     - Green: implement routes/storage until tests pass.
   - Acceptance: Can create game, join multiple players, retrieve lobby; invalid name/duplicate returns 4xx; state reflects joins.

3) Gift submission API
   - To Do: POST /game/{id}/gift (player id, product_url, hint); enforce one gift per player; allow edit before start; optional metadata preview stub behind flag; validation; DTOs for request/response.
   - TDD (Red → Green):
     - Red: tests for valid submission stored; duplicate before start overwrites/blocks per rule; submission after start rejected; invalid player rejected; metadata flag off returns placeholder; unknown game 404.
     - Green: implement handler/storage until tests pass.
   - Acceptance: Valid submission stored and returned in state; duplicate blocked or overwrites per rules; feature flag documented.

4) Start game and turn order
   - To Do: POST /game/{id}/start (host token); transition submissions -> in_progress; lock submissions; generate seeded/randomized turn order; set `current_turn`/`active_player`; emit initial events; expose game state DTO.
   - TDD (Red → Green):
     - Red: start without host token rejected; start with missing gifts rejected; seeded run yields stable turn order; start sets phase and active player; submissions blocked after start; unknown game 404.
     - Green: implement start logic until tests pass.
   - Acceptance: Start blocked without host token or missing gifts; turn order stable per seed in tests; submissions locked after start.

5) WebSocket hub for state fanout
   - To Do: WS /ws/{game_id}?player_id=...; authenticate game/player; send latest state on connect; process incoming actions through reducer; broadcast events/state; heartbeat/ping; backpressure policy; shared message formats.
   - TDD (Red → Green):
     - Red: integration test with two clients sees join state on connect; valid action updates both; invalid action returns error; disconnect cleanup works; unknown player/game rejects.
     - Green: implement hub and action handling until tests pass.
   - Acceptance: Multiple clients see synced state; invalid action rejected with error; idle connections kept alive; disconnect removes session safely.

6) Rule enforcement and edge cases
   - To Do: Enforce steal cap (max 3 per gift), no immediate steal-back, forced steal chains until new gift opened; guard rails for invalid phase/turn/gift; concurrency safety (mutex/queue around mutations); structured logging of rejects.
   - TDD (Red → Green):
     - Red: tests for steal cap reject on 4th attempt; immediate steal-back reject; forced chain continues correctly; simultaneous action attempts do not corrupt state.
     - Green: refine reducer/store locking until tests pass.
   - Acceptance: Tests for each rejection path; concurrent action attempts do not corrupt state; logs capture rule violations.

7) Optional persistence toggle
   - To Do: Feature flag/env to switch between in-memory store and Postgres-backed store; trait-based repository; migration stub for `games`, `players`, `gifts`, `events`; docs for envs.
   - TDD (Red → Green):
     - Red: repository trait tests for memory impl; with Postgres flag off, app boots in-memory; with flag on (mocked), routes use repo; migration command exists.
     - Green: implement repo + wiring until tests pass.
   - Acceptance: App runs with default in-memory; with Postgres flag set, can migrate and store/retrieve state; docs explain both paths.
