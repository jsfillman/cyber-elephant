use rand::seq::SliceRandom;
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

pub type PlayerId = String;
pub type GiftId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub joined_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GiftState {
    Unopened,
    Opened,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gift {
    pub id: GiftId,
    pub submitted_by: PlayerId,
    pub product_url: String,
    pub hint: String,
    pub image_url: Option<String>,
    pub title: Option<String>,
    pub opened_by: Option<PlayerId>,
    pub held_by: Option<PlayerId>,
    pub stolen_count: u8,
    pub state: GiftState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GamePhase {
    Lobby,
    Submissions,
    InProgress,
    Finished,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlayerAction {
    ChooseGift { player_id: PlayerId, gift_id: GiftId },
    StealGift { player_id: PlayerId, gift_id: GiftId },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum GameEvent {
    GiftOpened { player_id: PlayerId, gift_id: GiftId },
    GiftStolen { from: PlayerId, to: PlayerId, gift_id: GiftId },
    TurnChanged { player_id: PlayerId },
    GameFinished,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Game {
    pub id: String,
    pub phase: GamePhase,
    pub players: Vec<Player>,
    pub gifts: Vec<Gift>,
    pub turn_order: Vec<PlayerId>,
    pub current_turn: usize,
    pub active_player: Option<PlayerId>,
    pub history: Vec<GameEvent>,
}

impl Game {
    pub fn new(id: impl Into<String>, players: Vec<Player>, gifts: Vec<Gift>) -> Self {
        let turn_order = players.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
        let mut rng = thread_rng();
        let mut shuffled = turn_order.clone();
        shuffled.shuffle(&mut rng);

        let active = shuffled.first().cloned();

        Self {
            id: id.into(),
            phase: GamePhase::InProgress,
            players,
            gifts,
            turn_order: shuffled,
            current_turn: 0,
            active_player: active,
            history: Vec::new(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GameError {
    #[error("game not in progress")]
    WrongPhase,
    #[error("not your turn")]
    NotPlayersTurn,
    #[error("gift not found")]
    GiftNotFound,
    #[error("player not found")]
    PlayerNotFound,
    #[error("gift already opened")]
    GiftAlreadyOpened,
    #[error("gift unopened")]
    GiftUnopened,
    #[error("cannot steal your own gift")]
    CannotStealOwnGift,
    #[error("gift at steal limit")]
    StealLimitReached,
    #[error("immediate steal back not allowed")]
    StealBackNotAllowed,
    #[error("invalid action")]
    InvalidAction,
}

pub fn apply_action(game: &mut Game, action: PlayerAction) -> Result<Vec<GameEvent>, GameError> {
    if !matches!(game.phase, GamePhase::InProgress) {
        return Err(GameError::WrongPhase);
    }

    let actor = match &action {
        PlayerAction::ChooseGift { player_id, .. } => player_id,
        PlayerAction::StealGift { player_id, .. } => player_id,
    };

    let active_player = game.active_player.as_ref().ok_or(GameError::InvalidAction)?;
    if active_player != actor {
        return Err(GameError::NotPlayersTurn);
    }

    let mut events = Vec::new();
    match action.clone() {
        PlayerAction::ChooseGift { player_id, gift_id } => {
            choose_gift(game, &player_id, &gift_id, &mut events)?
        }
        PlayerAction::StealGift { player_id, gift_id } => {
            steal_gift(game, &player_id, &gift_id, &mut events)?
        }
    }

    // Check for game completion: all gifts opened and each player holds one.
    if all_gifts_opened(&game.gifts) && all_players_holding_one(&game.players, &game.gifts) {
        game.phase = GamePhase::Finished;
        events.push(GameEvent::GameFinished);
    }

    game.history.extend(events.clone());
    Ok(events)
}

fn choose_gift(
    game: &mut Game,
    player_id: &PlayerId,
    gift_id: &GiftId,
    events: &mut Vec<GameEvent>,
) -> Result<(), GameError> {
    let gift = game
        .gifts
        .iter_mut()
        .find(|g| &g.id == gift_id)
        .ok_or(GameError::GiftNotFound)?;

    if !matches!(gift.state, GiftState::Unopened) {
        return Err(GameError::GiftAlreadyOpened);
    }

    gift.state = GiftState::Opened;
    gift.opened_by = Some(player_id.clone());
    gift.held_by = Some(player_id.clone());
    events.push(GameEvent::GiftOpened {
        player_id: player_id.clone(),
        gift_id: gift_id.clone(),
    });

    advance_turn(game, events);
    Ok(())
}

fn steal_gift(
    game: &mut Game,
    player_id: &PlayerId,
    gift_id: &GiftId,
    events: &mut Vec<GameEvent>,
) -> Result<(), GameError> {
    let gift_index = game
        .gifts
        .iter()
        .position(|g| &g.id == gift_id)
        .ok_or(GameError::GiftNotFound)?;
    let current_state = game.gifts[gift_index].state.clone();
    let current_holder = game.gifts[gift_index]
        .held_by
        .clone()
        .ok_or(GameError::InvalidAction)?;
    let stolen_count = game.gifts[gift_index].stolen_count;

    if !matches!(current_state, GiftState::Opened) {
        return Err(GameError::GiftUnopened);
    }

    if current_holder == *player_id {
        return Err(GameError::CannotStealOwnGift);
    }

    if stolen_count >= 3 {
        return Err(GameError::StealLimitReached);
    }

    if immediate_steal_back(game, player_id, &current_holder) {
        return Err(GameError::StealBackNotAllowed);
    }

    let gift = game
        .gifts
        .get_mut(gift_index)
        .ok_or(GameError::GiftNotFound)?;

    gift.stolen_count += 1;
    gift.held_by = Some(player_id.clone());

    events.push(GameEvent::GiftStolen {
        from: current_holder.clone(),
        to: player_id.clone(),
        gift_id: gift_id.clone(),
    });

    // Forced steal chain: victim acts next; current_turn does not advance.
    game.active_player = Some(current_holder.clone());
    events.push(GameEvent::TurnChanged {
        player_id: current_holder,
    });

    Ok(())
}

fn advance_turn(game: &mut Game, events: &mut Vec<GameEvent>) {
    let next_index = game.current_turn + 1;
    if next_index < game.turn_order.len() {
        game.current_turn = next_index;
        let next_player = game.turn_order[next_index].clone();
        game.active_player = Some(next_player.clone());
        events.push(GameEvent::TurnChanged {
            player_id: next_player,
        });
    } else {
        // No more scheduled turns; remain at end and clear active player.
        game.active_player = None;
    }
}

fn immediate_steal_back(game: &Game, actor: &PlayerId, target: &PlayerId) -> bool {
    game.history
        .last()
        .map(|evt| match evt {
            GameEvent::GiftStolen { from, to, .. } => from == actor && to == target,
            _ => false,
        })
        .unwrap_or(false)
}

fn all_gifts_opened(gifts: &[Gift]) -> bool {
    gifts.iter().all(|g| matches!(g.state, GiftState::Opened))
}

fn all_players_holding_one(players: &[Player], gifts: &[Gift]) -> bool {
    let mut holder_counts: HashMap<&PlayerId, u8> = HashMap::new();
    for gift in gifts {
        if let Some(holder) = &gift.held_by {
            let count = holder_counts.entry(holder).or_insert(0);
            *count += 1;
        }
    }

    let required: HashSet<&PlayerId> = players.iter().map(|p| &p.id).collect();
    required.iter().all(|pid| holder_counts.get(pid) == Some(&1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(id: &str) -> Player {
        Player {
            id: id.to_string(),
            name: id.to_string(),
            joined_at: 0,
        }
    }

    fn unopened_gift(id: &str, submitted_by: &str) -> Gift {
        Gift {
            id: id.to_string(),
            submitted_by: submitted_by.to_string(),
            product_url: format!("https://example.com/{id}"),
            hint: format!("gift-{id}"),
            image_url: None,
            title: None,
            opened_by: None,
            held_by: None,
            stolen_count: 0,
            state: GiftState::Unopened,
        }
    }

    fn opened_gift(id: &str, owner: &str) -> Gift {
        Gift {
            id: id.to_string(),
            submitted_by: owner.to_string(),
            product_url: format!("https://example.com/{id}"),
            hint: format!("gift-{id}"),
            image_url: None,
            title: None,
            opened_by: Some(owner.to_string()),
            held_by: Some(owner.to_string()),
            stolen_count: 0,
            state: GiftState::Opened,
        }
    }

    fn base_game() -> Game {
        Game {
            id: "g1".into(),
            phase: GamePhase::InProgress,
            players: vec![player("p1"), player("p2"), player("p3")],
            gifts: vec![
                unopened_gift("g1", "p1"),
                unopened_gift("g2", "p2"),
                unopened_gift("g3", "p3"),
            ],
            turn_order: vec!["p1".into(), "p2".into(), "p3".into()],
            current_turn: 0,
            active_player: Some("p1".into()),
            history: vec![],
        }
    }

    #[test]
    fn open_gift_happy_path_advances_turn() {
        let mut game = base_game();
        let events = apply_action(
            &mut game,
            PlayerAction::ChooseGift {
                player_id: "p1".into(),
                gift_id: "g1".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            game.gifts[0].state,
            GiftState::Opened
        ));
        assert_eq!(game.gifts[0].held_by.as_deref(), Some("p1"));
        assert_eq!(game.current_turn, 1);
        assert_eq!(game.active_player.as_deref(), Some("p2"));
        assert_eq!(
            events,
            vec![
                GameEvent::GiftOpened {
                    player_id: "p1".into(),
                    gift_id: "g1".into()
                },
                GameEvent::TurnChanged {
                    player_id: "p2".into()
                }
            ]
        );
    }

    #[test]
    fn steal_happy_path_sets_victim_turn() {
        let mut game = base_game();
        // Pre-open two gifts to enable steal.
        game.gifts[0] = opened_gift("g1", "p1");
        game.gifts[1] = opened_gift("g2", "p2");
        game.current_turn = 1;
        game.active_player = Some("p2".into());

        assert_eq!(game.gifts[0].held_by.as_deref(), Some("p1"));

        let events = apply_action(
            &mut game,
            PlayerAction::StealGift {
                player_id: "p2".into(),
                gift_id: "g1".into(),
            },
        )
        .unwrap();

        assert_eq!(game.gifts[0].held_by.as_deref(), Some("p2"));
        assert_eq!(game.active_player.as_deref(), Some("p1"));
        assert_eq!(
            events,
            vec![
                GameEvent::GiftStolen {
                    from: "p1".into(),
                    to: "p2".into(),
                    gift_id: "g1".into()
                },
                GameEvent::TurnChanged {
                    player_id: "p1".into()
                }
            ]
        );
    }

    #[test]
    fn reject_over_steal_limit() {
        let mut game = base_game();
        game.gifts[0] = Gift {
            stolen_count: 3,
            ..opened_gift("g1", "p1")
        };
        game.active_player = Some("p2".into());
        game.current_turn = 1;

        let err = apply_action(
            &mut game,
            PlayerAction::StealGift {
                player_id: "p2".into(),
                gift_id: "g1".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err, GameError::StealLimitReached);
    }

    #[test]
    fn reject_immediate_steal_back() {
        let mut game = base_game();
        game.gifts[0] = opened_gift("g1", "p1");
        game.gifts[1] = opened_gift("g2", "p2");
        game.current_turn = 1;
        game.active_player = Some("p1".into());
        // Last action: p2 stole from p1
        game.history.push(GameEvent::GiftStolen {
            from: "p1".into(),
            to: "p2".into(),
            gift_id: "g1".into(),
        });

        game.gifts[0].held_by = Some("p2".into());
        game.gifts[0].stolen_count = 1;
        assert_eq!(game.gifts[0].held_by.as_deref(), Some("p2"));

        let err = apply_action(
            &mut game,
            PlayerAction::StealGift {
                player_id: "p1".into(),
                gift_id: "g1".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err, GameError::StealBackNotAllowed);
    }

    #[test]
    fn forced_chain_advances_when_new_gift_opened() {
        let mut game = base_game();
        // A steals from B, so B acts next.
        game.gifts[0] = opened_gift("g1", "p1");
        game.gifts[1] = opened_gift("g2", "p2");
        game.current_turn = 1;
        game.active_player = Some("p2".into());

        apply_action(
            &mut game,
            PlayerAction::StealGift {
                player_id: "p2".into(),
                gift_id: "g1".into(),
            },
        )
        .unwrap();
        // Victim is p1 now active; they open a fresh gift.
        let events = apply_action(
            &mut game,
            PlayerAction::ChooseGift {
                player_id: "p1".into(),
                gift_id: "g3".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            game.gifts[2].state,
            GiftState::Opened
        ));
        assert_eq!(game.current_turn, 2);
        assert_eq!(game.active_player.as_deref(), Some("p3"));
        assert_eq!(
            events.last(),
            Some(&GameEvent::TurnChanged {
                player_id: "p3".into()
            })
        );
    }

    #[test]
    fn rejects_wrong_turn_or_phase() {
        let mut game = base_game();
        game.phase = GamePhase::Lobby;
        let err = apply_action(
            &mut game,
            PlayerAction::ChooseGift {
                player_id: "p1".into(),
                gift_id: "g1".into(),
            },
        )
        .unwrap_err();
        assert_eq!(err, GameError::WrongPhase);

        game.phase = GamePhase::InProgress;
        game.active_player = Some("p1".into());
        let err = apply_action(
            &mut game,
            PlayerAction::ChooseGift {
                player_id: "p2".into(),
                gift_id: "g1".into(),
            },
        )
        .unwrap_err();
        assert_eq!(err, GameError::NotPlayersTurn);
    }
}
