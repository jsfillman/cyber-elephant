use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::env;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use game_core::{Game, GameEvent, GamePhase, Gift as CoreGift, GiftState, Player as CorePlayer, PlayerAction};
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use uuid::Uuid;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use tokio::sync::broadcast;
use futures::StreamExt;
use futures::SinkExt;

#[derive(Clone)]
pub struct AppState {
    games: Arc<RwLock<HashMap<String, GameRecord>>>,
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<ServerMessage>>>>,
    persist_path: Option<PathBuf>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            games: Arc::new(RwLock::new(HashMap::new())),
            channels: Arc::new(RwLock::new(HashMap::new())),
            persist_path: None,
        }
    }
}

impl AppState {
    pub async fn with_persistence(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut state = Self::default();
        state.persist_path = Some(path.clone());
        if let Ok(bytes) = tokio::fs::read(&path).await {
            if let Ok(saved) = serde_json::from_slice::<HashMap<String, GameRecord>>(&bytes) {
                let mut games = state.games.write().await;
                *games = saved;
                let mut channels = state.channels.write().await;
                for game_id in games.keys() {
                    let (tx, _) = broadcast::channel(32);
                    channels.insert(game_id.clone(), tx);
                }
            }
        }
        state
    }

    async fn persist(&self) {
        if let Some(path) = &self.persist_path {
            let snapshot = {
                let games = self.games.read().await;
                games.clone()
            };
            if let Ok(json) = serde_json::to_vec_pretty(&snapshot) {
                if let Err(err) = tokio::fs::write(path, json).await {
                    eprintln!("persist error: {err}");
                }
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameRecord {
    pub id: String,
    pub host_token: String,
    pub players: Vec<PlayerRecord>,
    pub gifts: Vec<GiftRecord>,
    pub phase: GamePhase,
    pub turn_order: Vec<String>,
    pub current_turn: usize,
    pub active_player: Option<String>,
    pub history: Vec<GameEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerRecord {
    pub id: String,
    pub name: String,
    pub joined_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GiftRecord {
    pub id: String,
    pub submitted_by: String,
    pub product_url: String,
    pub hint: String,
    pub image_url: Option<String>,
    pub title: Option<String>,
    pub opened_by: Option<String>,
    pub held_by: Option<String>,
    pub stolen_count: u8,
    pub state: GiftState,
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/game", post(create_game))
        .route("/game/:id/join", post(join_game))
        .route("/game/:id/gift", post(submit_gift))
        .route("/game/:id/start", post(start_game))
        .route("/ws/:id/:player_id", get(ws_handler))
        .route("/game/:id", get(get_game))
        .with_state(state)
}

#[derive(Serialize)]
struct CreateGameResponse {
    game_id: String,
    host_token: String,
}

fn admin_password() -> String {
    env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "changeme".to_string())
}

async fn create_game(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let expected = admin_password();
    let provided = headers
        .get("x-admin-password")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided != expected {
        return (StatusCode::UNAUTHORIZED, "invalid admin password").into_response();
    }

    let game_id = Uuid::new_v4().to_string();
    let host_token = Uuid::new_v4().to_string();
    let record = GameRecord {
        id: game_id.clone(),
        host_token: host_token.clone(),
        players: Vec::new(),
        gifts: Vec::new(),
        phase: GamePhase::Submissions,
        turn_order: Vec::new(),
        current_turn: 0,
        active_player: None,
        history: Vec::new(),
    };

    state.games.write().await.insert(game_id.clone(), record);
    let (tx, _) = broadcast::channel(32);
    state.channels.write().await.insert(game_id.clone(), tx);
    state.persist().await;

    (
        StatusCode::CREATED,
        Json(CreateGameResponse {
            game_id,
            host_token,
        }),
    )
        .into_response()
}

#[derive(Deserialize)]
struct JoinRequest {
    name: String,
}

#[derive(Serialize)]
struct JoinResponse {
    player_id: String,
}

#[derive(Deserialize)]
struct GiftRequest {
    player_id: String,
    product_url: String,
    hint: String,
    image_url: Option<String>,
    title: Option<String>,
}

#[derive(Serialize)]
struct GiftResponse {
    gift: GiftRecord,
}

#[derive(Deserialize)]
struct StartParams {
    seed: Option<u64>,
}

#[derive(Serialize)]
struct StartResponse {
    phase: GamePhase,
    turn_order: Vec<String>,
    active_player: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    State(GameView),
    Event(GameEvent),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Action(PlayerAction),
}

async fn join_game(
    State(state): State<AppState>,
    Path(game_id): Path<String>,
    Json(payload): Json<JoinRequest>,
) -> impl IntoResponse {
    let name = payload.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }

    let mut games = state.games.write().await;
    let game = match games.get_mut(&game_id) {
        Some(game) => game,
        None => return (StatusCode::NOT_FOUND, "game not found").into_response(),
    };

    if game.players.iter().any(|p| p.name == name) {
        return (StatusCode::CONFLICT, "name taken").into_response();
    }

    let player_id = Uuid::new_v4().to_string();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    game.players.push(PlayerRecord {
        id: player_id.clone(),
        name: name.to_string(),
        joined_at: now,
    });

    drop(games);
    state.persist().await;

    (StatusCode::OK, Json(JoinResponse { player_id })).into_response()
}

async fn submit_gift(
    State(state): State<AppState>,
    Path(game_id): Path<String>,
    Json(payload): Json<GiftRequest>,
) -> impl IntoResponse {
    let mut games = state.games.write().await;
    let game = match games.get_mut(&game_id) {
        Some(g) => g,
        None => return (StatusCode::NOT_FOUND, "game not found").into_response(),
    };

    if !matches!(game.phase, GamePhase::Submissions) {
        return (StatusCode::CONFLICT, "submissions closed").into_response();
    }

    if payload.product_url.trim().is_empty() || payload.hint.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "product_url and hint required").into_response();
    }

    if !game.players.iter().any(|p| p.id == payload.player_id) {
        return (StatusCode::NOT_FOUND, "player not found").into_response();
    }

    let existing = game
        .gifts
        .iter_mut()
        .find(|g| g.submitted_by == payload.player_id);

    let gift_record = if let Some(g) = existing {
        g.product_url = payload.product_url.clone();
        g.hint = payload.hint.clone();
        g.image_url = payload.image_url.clone();
        g.title = payload.title.clone();
        g.clone()
    } else {
        let gift = GiftRecord {
            id: Uuid::new_v4().to_string(),
            submitted_by: payload.player_id.clone(),
            product_url: payload.product_url.clone(),
            hint: payload.hint.clone(),
            image_url: payload.image_url.clone(),
            title: payload.title.clone(),
            opened_by: None,
            held_by: None,
            stolen_count: 0,
            state: GiftState::Unopened,
        };
        game.gifts.push(gift.clone());
        gift
    };

    drop(games);
    state.persist().await;

    (StatusCode::OK, Json(GiftResponse { gift: gift_record })).into_response()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GameView {
    id: String,
    phase: GamePhase,
    players: Vec<PlayerRecord>,
    gifts: Vec<GiftRecord>,
    turn_order: Vec<String>,
    active_player: Option<String>,
}

async fn get_game(
    State(state): State<AppState>,
    Path(game_id): Path<String>,
) -> impl IntoResponse {
    let games = state.games.read().await;
    let Some(game) = games.get(&game_id) else {
        return (StatusCode::NOT_FOUND, "game not found").into_response();
    };

    (
        StatusCode::OK,
        Json(GameView {
            id: game.id.clone(),
            phase: game.phase.clone(),
            players: game.players.clone(),
            gifts: game.gifts.clone(),
            turn_order: game.turn_order.clone(),
            active_player: game.active_player.clone(),
        }),
    )
        .into_response()
}

async fn start_game(
    State(state): State<AppState>,
    Path(game_id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<StartParams>,
) -> impl IntoResponse {
    let mut games = state.games.write().await;
    let game = match games.get_mut(&game_id) {
        Some(g) => g,
        None => return (StatusCode::NOT_FOUND, "game not found").into_response(),
    };

    let Some(token_val) = headers.get("x-host-token").and_then(|v| v.to_str().ok()) else {
        return (StatusCode::UNAUTHORIZED, "host token required").into_response();
    };

    if token_val != game.host_token {
        return (StatusCode::UNAUTHORIZED, "invalid host token").into_response();
    }

    if !matches!(game.phase, GamePhase::Submissions) {
        return (StatusCode::CONFLICT, "game already started").into_response();
    }

    if game.players.is_empty() {
        return (StatusCode::BAD_REQUEST, "no players").into_response();
    }

    let all_have_gifts = game
        .players
        .iter()
        .all(|p| game.gifts.iter().any(|g| g.submitted_by == p.id));

    if !all_have_gifts {
        return (StatusCode::BAD_REQUEST, "all players must submit gifts").into_response();
    }

    let mut turn_order = game.players.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    let mut rng = params
        .seed
        .map(ChaCha8Rng::seed_from_u64)
        .unwrap_or_else(|| ChaCha8Rng::from_entropy());
    turn_order.shuffle(&mut rng);

    game.phase = GamePhase::InProgress;
    game.turn_order = turn_order.clone();
    game.current_turn = 0;
    game.active_player = turn_order.first().cloned();
    game.history.clear();
    for gift in game.gifts.iter_mut() {
        gift.state = GiftState::Unopened;
        gift.opened_by = None;
        gift.held_by = None;
        gift.stolen_count = 0;
    }

    let response = (
        StatusCode::OK,
        Json(StartResponse {
            phase: game.phase.clone(),
            turn_order,
            active_player: game.active_player.clone(),
        }),
    )
        .into_response();

    drop(games);
    state.persist().await;

    response
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path((game_id, player_id)): Path<(String, String)>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, game_id, player_id))
}

async fn handle_socket(stream: WebSocket, state: AppState, game_id: String, player_id: String) {
    let (sender, mut receiver) = stream.split();
    let sender = Arc::new(tokio::sync::Mutex::new(sender));

    // Fetch game and channel
    let snapshot = {
        let games = state.games.read().await;
        let game = match games.get(&game_id) {
            Some(g) => g.clone(),
            None => {
                let _ = sender
                    .lock()
                    .await
                    .send(Message::Text("unknown game".into()))
                    .await;
                return;
            }
        };
        if !game.players.iter().any(|p| p.id == player_id) {
            let _ = sender
                .lock()
                .await
                .send(Message::Text("unknown player".into()))
                .await;
            return;
        }
        GameView {
            id: game.id.clone(),
            phase: game.phase.clone(),
            players: game.players.clone(),
            gifts: game.gifts.clone(),
            turn_order: game.turn_order.clone(),
            active_player: game.active_player.clone(),
        }
    };

    let rx = {
        let mut channels = state.channels.write().await;
        channels
            .entry(game_id.clone())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(32);
                tx
            })
            .subscribe()
    };

    // Send snapshot
    let _ = sender
        .lock()
        .await
        .send(Message::Text(
            serde_json::to_string(&ServerMessage::State(snapshot)).unwrap(),
        ))
        .await;

    // Task to forward broadcasts
    let sender_clone = sender.clone();
    let mut send_task = tokio::spawn(async move {
        let mut rx = rx;
        while let Ok(msg) = rx.recv().await {
            if sender_clone
                .lock()
                .await
                .send(Message::Text(serde_json::to_string(&msg).unwrap()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let state_clone = state.clone();
    let sender_err = sender.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = receiver.next().await {
            if let Ok(ClientMessage::Action(action)) = serde_json::from_str(&text) {
                if let Err(e) =
                    process_action(&state_clone, &game_id, &player_id, action.clone()).await
                {
                    let _ = sender_err
                        .lock()
                        .await
                        .send(Message::Text(format!("error:{e:?}")))
                        .await;
                }
            }
        }
    });

    let _ = (&mut send_task).await;
    recv_task.abort();
}

async fn process_action(
    state: &AppState,
    game_id: &str,
    player_id: &str,
    action: PlayerAction,
) -> Result<(), GameActionError> {
    let mut games = state.games.write().await;
    let game_record = games
        .get_mut(game_id)
        .ok_or(GameActionError::GameNotFound)?;

    if !matches!(game_record.phase, GamePhase::InProgress) {
        return Err(GameActionError::WrongPhase);
    }

    if !game_record.players.iter().any(|p| p.id == player_id) {
        return Err(GameActionError::PlayerNotFound);
    }

    let mut core_game = to_core(game_record.clone());
    let events = game_core::apply_action(&mut core_game, action)?;
    // update record from core
    update_record_from_core(game_record, core_game);

    // broadcast state + events
    if let Some(tx) = state.channels.read().await.get(game_id) {
        let _ = tx.send(ServerMessage::State(to_view(game_record)));
        for evt in events {
            let _ = tx.send(ServerMessage::Event(evt));
        }
    }
    drop(games);
    state.persist().await;

    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum GameActionError {
    #[error("game not found")]
    GameNotFound,
    #[error("player not found")]
    PlayerNotFound,
    #[error("wrong phase")]
    WrongPhase,
    #[error("core error: {0}")]
    Core(#[from] game_core::GameError),
}

fn to_core(record: GameRecord) -> Game {
    Game {
        id: record.id,
        phase: record.phase,
        players: record
            .players
            .iter()
            .map(|p| CorePlayer {
                id: p.id.clone(),
                name: p.name.clone(),
                joined_at: p.joined_at,
            })
            .collect(),
        gifts: record
            .gifts
            .iter()
            .map(|g| CoreGift {
                id: g.id.clone(),
                submitted_by: g.submitted_by.clone(),
                product_url: g.product_url.clone(),
                hint: g.hint.clone(),
                image_url: g.image_url.clone(),
                title: g.title.clone(),
                opened_by: g.opened_by.clone(),
                held_by: g.held_by.clone(),
                stolen_count: g.stolen_count,
                state: g.state.clone(),
            })
            .collect(),
        turn_order: record.turn_order.clone(),
        current_turn: record.current_turn,
        active_player: record.active_player.clone(),
        history: record.history.clone(),
    }
}

fn update_record_from_core(record: &mut GameRecord, core: Game) {
    record.phase = core.phase;
    record.turn_order = core.turn_order;
    record.current_turn = core.current_turn;
    record.active_player = core.active_player;
    record.history = core.history;
    record.gifts = core
        .gifts
        .into_iter()
        .map(|g| GiftRecord {
            id: g.id,
            submitted_by: g.submitted_by,
            product_url: g.product_url,
            hint: g.hint,
            image_url: g.image_url,
            title: g.title,
            opened_by: g.opened_by,
            held_by: g.held_by,
            stolen_count: g.stolen_count,
            state: g.state,
        })
        .collect();
}

fn to_view(game: &GameRecord) -> GameView {
    GameView {
        id: game.id.clone(),
        phase: game.phase.clone(),
        players: game.players.clone(),
        gifts: game.gifts.clone(),
        turn_order: game.turn_order.clone(),
        active_player: game.active_player.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use http_body_util::BodyExt;
    use rand::seq::SliceRandom;
    use serde_json::json;
    use tower::ServiceExt;

    async fn json_body(res: axum::response::Response) -> serde_json::Value {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn test_app() -> (Router, AppState) {
        let state = AppState::default();
        (app(state.clone()), state)
    }

    #[tokio::test]
    async fn create_game_returns_ids() {
        let (app, _) = test_app();
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/game")
                    .header("x-admin-password", "changeme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::CREATED);
        let body = json_body(res).await;
        assert!(body.get("game_id").and_then(|v| v.as_str()).is_some());
        assert!(body.get("host_token").and_then(|v| v.as_str()).is_some());
    }

    #[tokio::test]
    async fn create_game_requires_admin_password() {
        let (app, _) = test_app();
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/game")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn join_success_and_duplicate_name_rejected() {
        let (app, _state) = test_app();
        // create game
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/game")
                    .header("x-admin-password", "changeme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let created = json_body(res).await;
        let game_id = created["game_id"].as_str().unwrap();

        // join first player
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/game/{game_id}/join"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "name": "alice" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let join_body = json_body(res).await;
        assert!(join_body["player_id"].as_str().is_some());

        // duplicate name rejected
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/game/{game_id}/join"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "name": "alice" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);

        // lobby order preserved
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/game/{game_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let lobby = json_body(res).await;
        let players = lobby["players"].as_array().unwrap();
        assert_eq!(players.len(), 1);
        assert_eq!(players[0]["name"], "alice");

        // unknown game 404
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/game/unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/game/unknown/join")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "name": "bob" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn gift_submission_upserts_before_start_and_blocks_after() {
        let (app, _) = test_app();
        let created = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/game")
                        .header("x-admin-password", "changeme")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let game_id = created["game_id"].as_str().unwrap();
        let host_token = created["host_token"].as_str().unwrap();

        // join player
        let join_body = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/game/{game_id}/join"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "name": "alice" }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let player_id = join_body["player_id"].as_str().unwrap();

        // submit first gift
        let gift1 = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/game/{game_id}/gift"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "player_id": player_id, "product_url": "https://example.com/1", "hint": "first" }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let gift_id = gift1["gift"]["id"].as_str().unwrap();
        assert_eq!(gift1["gift"]["hint"], "first");

        // resubmit to edit before start
        let gift2 = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/game/{game_id}/gift"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "player_id": player_id, "product_url": "https://example.com/2", "hint": "updated" }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(gift2["gift"]["id"].as_str().unwrap(), gift_id);
        assert_eq!(gift2["gift"]["hint"], "updated");

        // need another player with gift to start
        let join_body = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/game/{game_id}/join"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "name": "bob" }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let player_bob = join_body["player_id"].as_str().unwrap();
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/game/{game_id}/gift"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "player_id": player_bob, "product_url": "https://example.com/3", "hint": "bob gift" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        // start game
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/game/{game_id}/start"))
                    .header("x-host-token", host_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // submissions blocked after start
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/game/{game_id}/gift"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "player_id": player_id, "product_url": "https://example.com/4", "hint": "nope" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn start_requires_host_token_and_all_gifts_with_seeded_order() {
        let (app, _) = test_app();
        let created = json_body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/game")
                        .header("x-admin-password", "changeme")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let game_id = created["game_id"].as_str().unwrap();
        let host_token = created["host_token"].as_str().unwrap();

        // join three players
        let mut player_ids = Vec::new();
        for name in &["alice", "bob", "carol"] {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/game/{game_id}/join"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "name": name }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let body = json_body(res).await;
            player_ids.push(body["player_id"].as_str().unwrap().to_string());
        }

        // missing host token
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/game/{game_id}/start"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // missing gifts
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/game/{game_id}/start"))
                    .header("x-host-token", host_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // submit gifts for all
        for pid in &player_ids {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/game/{game_id}/gift"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "player_id": pid, "product_url": format!("https://example.com/{pid}"), "hint": format!("gift-{pid}") }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
        }

        // start with seed for deterministic order
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/game/{game_id}/start?seed=42"))
                    .header("x-host-token", host_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json_body(res).await;
        assert_eq!(body["phase"], "in_progress");
        let turn_order = body["turn_order"].as_array().unwrap();

        let mut expected = player_ids.clone();
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(42);
        expected.shuffle(&mut rng);

        let returned: Vec<String> = turn_order
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(returned, expected);
        assert_eq!(body["active_player"].as_str().unwrap(), expected[0]);
    }

    #[tokio::test]
    async fn persistence_writes_and_loads_games() {
        let path = std::env::temp_dir().join(format!("ce_state_{}.json", Uuid::new_v4()));
        let state = AppState::with_persistence(path.clone()).await;
        let app = app(state.clone());

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/game")
                    .header("x-admin-password", "changeme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);

        // ensure file exists
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(tokio::fs::metadata(&path).await.is_ok());

        // load new state from disk
        let loaded = AppState::with_persistence(path.clone()).await;
        let games = loaded.games.read().await;
        assert_eq!(games.len(), 1);
    }
}
