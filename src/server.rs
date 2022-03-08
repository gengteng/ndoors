use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Extension, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use dashmap::DashMap;
use ndoors::*;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = SocketAddr::new([0, 0, 0, 0].into(), 7654);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .layer(Extension(Server::default()));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Extension(server): Extension<Server>,
) -> impl IntoResponse {
    ws.on_upgrade(|s| async move {
        if let Err(e) = handle_ws(s, server).await {
            eprintln!("Websocket error: {e}");
        }
    })
}

#[derive(Debug, Clone)]
struct Server {
    rooms: Arc<DashMap<Uuid, (Room, Sender<InternalRequest>)>>,
    default_settings: Settings,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            rooms: Default::default(),
            default_settings: Settings::new(3, 10),
        }
    }
}

#[derive(Debug)]
struct User {
    id: Uuid,
    role: Role,
}

impl Default for User {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            role: Role::Guest,
        }
    }
}

#[derive(Debug)]
enum Role {
    Guest,
    Host { sender: Sender<InternalRequest> },
    Contestant { sender: Sender<InternalRequest> },
}

async fn handle_ws(mut socket: WebSocket, server: Server) -> anyhow::Result<()> {
    let mut user = User::default();

    // 监听 socket 以及 room 中其他成员广播的消息
    loop {
        if let Some(result) = socket.recv().await.transpose()? {
            match result {
                Message::Text(request) => {
                    let request: GameRequest = serde_json::from_str(&request)?;
                    match (request, &mut user) {
                        (GameRequest::ListRooms { page, size }, _) => {
                            let total = server.rooms.len() as u32;
                            let rooms = server
                                .rooms
                                .iter()
                                .skip((page * size) as usize)
                                .map(|room| (room.0.id().clone(), room.0.settings()))
                                .collect();
                            let response = GameResponse::RoomList {
                                rooms,
                                page,
                                size,
                                total,
                            };
                            socket
                                .send(Message::Text(serde_json::to_string(&response)?))
                                .await?;
                        }
                        (GameRequest::EnterRoom { id }, user) => {
                            let response = match user.role {
                                Role::Guest => match server.rooms.get_mut(&id) {
                                    None => GameResponse::ServerError {
                                        cause: ServerError::RoomNotFound { id },
                                    },
                                    Some(mut room) => {
                                        room.0.accept_contestant(user.id)?;
                                        user.role = Role::Contestant {
                                            sender: room.1.clone(),
                                        };
                                        GameResponse::RoomEntered
                                    }
                                },
                                _ => GameResponse::GameError {
                                    cause: Error::InvalidOperation,
                                },
                            };
                            // TODO: 广播给主持人
                            socket
                                .send(Message::Text(serde_json::to_string(&response)?))
                                .await?;
                        }
                        (GameRequest::CreateRoom { settings }, user) => {
                            let response = match user.role {
                                Role::Guest => {
                                    let settings = match settings {
                                        None => server.default_settings,
                                        Some(settings) => settings,
                                    };

                                    let room = Room::create(user.id, settings);
                                    let room_id = room.id().clone();
                                    let (sender, receiver) = channel(16);
                                    server.rooms.insert(room_id, (room, sender.clone()));
                                    let s = server.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = room_loop(room_id, s, receiver).await {
                                            eprintln!("Room loop error: {e}");
                                        }
                                    });
                                    user.role = Role::Host { sender };
                                    GameResponse::RoomCreated {
                                        id: room_id,
                                        settings,
                                    }
                                }
                                _ => GameResponse::GameError {
                                    cause: Error::InvalidOperation,
                                },
                            };
                            socket
                                .send(Message::Text(serde_json::to_string(&response)?))
                                .await?;
                        }
                        (request, user) => {
                            let response = match &user.role {
                                Role::Contestant { sender } | Role::Host { sender } => {
                                    let (responder, rx) = oneshot::channel();
                                    sender
                                        .send(InternalRequest { request, responder })
                                        .await
                                        .map_err(send_error)?;
                                    match rx.await? {
                                        // 接收出错意味着房间挂了？
                                        Ok(response) => response,
                                        Err(cause) => GameResponse::GameError { cause },
                                    }
                                }
                                _ => GameResponse::GameError {
                                    cause: Error::InvalidOperation,
                                },
                            };

                            // TODO: 广播给所有人，奖品索引不要发送给挑战者
                            socket
                                .send(Message::Text(serde_json::to_string(&response)?))
                                .await?;
                        }
                    }
                }
                Message::Close(c) => match c {
                    Some(c) => {
                        println!(
                            "Connection closed: code = {}, reason = {}",
                            c.code, c.reason
                        );
                        break;
                    }
                    None => {
                        println!("Connection closed without close frame",);
                        break;
                    }
                },
                _ => {}
            }
        } else {
            break;
        }
    }

    Ok(())
}

async fn room_loop(
    id: Uuid,
    server: Server,
    mut receiver: Receiver<InternalRequest>,
) -> anyhow::Result<()> {
    loop {
        match receiver.recv().await {
            Some(request) => {
                let InternalRequest { request, responder } = request;
                let room = &mut server
                    .rooms
                    .get_mut(&id)
                    .ok_or_else(|| anyhow::anyhow!("Room {} closed.", id))?
                    .0;
                match request {
                    GameRequest::ExitRoom { id } => {
                        if id != *room.id() {
                            eprintln!("exit room error: {} != {}", id, room.id())
                        }
                        responder
                            .send(Ok(GameResponse::Exited))
                            .map_err(send_error)?;
                        break;
                    }
                    GameRequest::Ready { ready } => {
                        let response = room
                            .contestant_ready(ready)
                            .map(|_| GameResponse::Ready { ready });
                        responder.send(response).map_err(send_error)?;
                    }
                    GameRequest::Choose { chosen } => {
                        let response = match chosen {
                            Index::Random => {
                                room.choose_random().map(|chosen| GameResponse::Chosen {
                                    chosen,
                                    random: true,
                                })
                            }
                            Index::Specified(chosen) => {
                                room.choose(chosen).map(|_| GameResponse::Chosen {
                                    chosen,
                                    random: false,
                                })
                            }
                        };
                        responder.send(response).map_err(send_error)?;
                    }
                    GameRequest::Decide { decision } => {
                        let response = room
                            .decide(decision)
                            .map(|result| GameResponse::Decided { result });
                        responder.send(response).map_err(send_error)?;
                    }
                    GameRequest::UpdateSettings { settings } => {
                        let response = room
                            .update_settings(settings)
                            .map(|notify| GameResponse::SettingsUpdated { settings, notify });
                        responder.send(response).map_err(send_error)?;
                    }
                    GameRequest::Start { prize } => {
                        let response = match prize {
                            Index::Random => {
                                room.start_random().map(|prize| GameResponse::Started {
                                    prize,
                                    random: true,
                                })
                            }
                            Index::Specified(prize) => {
                                room.start(prize).map(|_| GameResponse::Started {
                                    prize,
                                    random: false,
                                })
                            }
                        };

                        responder.send(response).map_err(send_error)?;
                    }
                    GameRequest::Reveal { left } => {
                        let response = match left {
                            Index::Random => room
                                .reveal_random()
                                .map(|left| GameResponse::Revealed { left, random: true }),
                            Index::Specified(left) => {
                                room.reveal(left).map(|_| GameResponse::Revealed {
                                    left,
                                    random: false,
                                })
                            }
                        };

                        responder.send(response).map_err(send_error)?;
                    }
                    GameRequest::Complete { kick_contestant } => {
                        let response = room.complete(kick_contestant).map(|results| {
                            let result = GameResult::calculate(room.settings().doors, results);
                            GameResponse::Completed { result }
                        });
                        responder.send(response).map_err(send_error)?;
                    }
                    _req => {}
                }
            }
            None => {
                break;
            }
        }
    }

    Ok(())
}

fn send_error<T>(_: T) -> anyhow::Error {
    anyhow::anyhow!("Failed to respond: channel closed.")
}

#[derive(Debug)]
struct InternalRequest {
    request: GameRequest,
    responder: oneshot::Sender<Result<GameResponse>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action")]
enum GameRequest {
    ListRooms { page: u32, size: u32 },
    EnterRoom { id: Uuid },
    ExitRoom { id: Uuid },
    Ready { ready: bool },
    Choose { chosen: Index },
    Decide { decision: Decision },
    CreateRoom { settings: Option<Settings> },
    UpdateSettings { settings: Settings },
    Start { prize: Index },
    Reveal { left: Index },
    Complete { kick_contestant: bool },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum Index {
    Random,
    Specified(u32),
}

#[derive(thiserror::Error, Debug, Serialize, Deserialize)]
enum ServerError {
    #[error("Room not found: {}", .id)]
    RoomNotFound { id: Uuid },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "resp")]
enum GameResponse {
    RoomList {
        rooms: Vec<(Uuid, Settings)>,
        page: u32,
        size: u32,
        total: u32,
    },
    RoomCreated {
        id: Uuid,
        settings: Settings,
    },
    Exited,
    RoomEntered,
    SettingsUpdated {
        notify: bool,
        settings: Settings,
    },
    Ready {
        ready: bool,
    },
    Started {
        prize: u32,
        random: bool,
    },
    ContestantStarted {
        random: bool,
    },
    Chosen {
        chosen: u32,
        random: bool,
    },
    Revealed {
        left: u32,
        random: bool,
    },
    Decided {
        result: RoundResult,
    },
    Completed {
        result: GameResult,
    },
    GameError {
        cause: Error,
    },
    ServerError {
        cause: ServerError,
    },
}
