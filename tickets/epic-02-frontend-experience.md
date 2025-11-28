# Epic 02 - Realtime Frontend Experience

Goal: Deliver a slick SPA with lobby, gift submission, and live game board, all synced over WebSockets.
Outcome: Players can join, submit, and play with responsive UI, inline validation, and the “hella cool” visual polish.

## Stories
1) Frontend scaffold and design system
   - Initialize React + Vite + TypeScript + Tailwind (or SvelteKit if preferred; pick one and document).
   - Set up base layout, typography scale, color tokens, and Tailwind config.
   - Add iconography and motion primitives (e.g., framer-motion or CSS utilities).

2) Lobby screen with host join link
   - Join form with name input and simple validation.
   - Display current players list in real time from REST/WS.
   - Error toasts for join failures (name taken, lobby full).

3) Gift submission flow
   - Form for `product_url` + `hint` with client-side validation.
   - Preview card using provided metadata; fallback placeholder when metadata not available.
   - Submission success state and ability to edit before start if allowed by backend.

4) WebSocket client and state store
   - Client that connects with game id + player id; retries with backoff.
   - Normalized store for `GameState` and `GameEvent`s; optimistic UI disabled (server authoritative).
   - Loading/empty/error UI states for connect/disconnect.

5) Game board and turn actions
   - Board showing gifts (unopened/opened/owned), active player banner, and turn queue.
   - Buttons for “Choose” or “Steal” gated by rule validation from backend state.
   - Gift reveal modal with image/title/hint; steal target selection UI.

6) Animations and delight
   - Motion for gift open (scale/opacity) and steal (wiggle/bounce + ownership highlight).
   - Staggered list reveals for lobby/board; smooth state transitions.
   - Lightweight sound hooks ready (can be muted); keep performant on mobile.

7) Responsive and accessibility pass
   - Mobile-first layout with sensible breakpoints.
   - Keyboard focus management for modals/buttons; aria labels for controls.
   - Color contrast and reduced-motion support.
