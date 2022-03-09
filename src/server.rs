use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Extension, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, get_service};
use axum::Router;
use dashmap::DashMap;
use ndoors::*;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = SocketAddr::new([0, 0, 0, 0].into(), 7654);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .fallback(get_service(ServeDir::new("./html")).handle_error(
            |error: std::io::Error| async move {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Unhandled internal error: {}", error),
                )
            },
        ))
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
    ws.on_upgrade(|socket| async move {
        let (resp_sender, resp_receiver) = channel(16);
        let (req_sender, req_receiver) = channel(16);
        let user = User::new(resp_sender);
        if user
            .sender
            .send(GameResponse::UserCreated { id: user.id })
            .await
            .is_err()
        {
            eprintln!("Failed to send UserCreated response");
            return;
        }

        let s = server.clone();
        tokio::spawn(async move {
            if let Err(e) = request_handler(user, s, req_receiver).await {
                eprintln!("Request handler error: {e}");
            }
        });
        if let Err(e) = websocket_loop(socket, req_sender, resp_receiver).await {
            eprintln!("Websocket error: {e}");
        }
    })
}

#[derive(Debug, Clone)]
struct Server {
    rooms: Arc<DashMap<Uuid, RoomAgent>>,
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
struct RoomAgent {
    room: Room,
    host: Sender<GameResponse>,
    contestant: Option<Sender<GameResponse>>,
}

impl RoomAgent {
    pub async fn publish(&self, response: GameResponse) -> anyhow::Result<()> {
        self.host.send(response.clone()).await.map_err(send_error)?;
        if let Some(contestant) = &self.contestant {
            contestant.send(response).await.map_err(send_error)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct User {
    id: Uuid,
    role: Role,
    sender: Sender<GameResponse>,
}

impl User {
    pub fn new(sender: Sender<GameResponse>) -> Self {
        Self {
            id: Uuid::new_v4(),
            role: Role::Guest,
            sender,
        }
    }
}

#[derive(Debug)]
enum Role {
    Guest,
    Host { room_id: Uuid },
    Contestant { room_id: Uuid },
}

async fn request_handler(
    mut user: User,
    server: Server,
    mut receiver: Receiver<GameRequest>,
) -> anyhow::Result<()> {
    while let Some(request) = receiver.recv().await {
        match (request, &mut user) {
            (GameRequest::ListRooms { page, size }, _) => {
                let total = server.rooms.len() as u32;
                let rooms = server
                    .rooms
                    .iter()
                    .skip((page * size) as usize)
                    .map(|ra| (*ra.room.id(), ra.room.settings()))
                    .collect();
                let response = GameResponse::RoomList {
                    rooms,
                    page,
                    size,
                    total,
                };
                user.sender.send(response).await.map_err(send_error)?;
            }
            (GameRequest::EnterRoom { id }, user) => {
                match user.role {
                    Role::Guest => match server.rooms.get_mut(&id) {
                        None => {
                            let response = GameResponse::ServerError {
                                cause: ServerError::RoomNotFound { id },
                            };
                            user.sender.send(response).await.map_err(send_error)?;
                        }
                        Some(mut ra) => {
                            ra.room.accept_contestant(user.id)?;
                            ra.contestant = Some(user.sender.clone());

                            user.role = Role::Contestant {
                                room_id: *ra.room.id(),
                            };

                            ra.publish(GameResponse::RoomEntered).await?;
                        }
                    },
                    _ => {
                        let response = GameResponse::GameError {
                            cause: Error::InvalidOperation,
                        };
                        user.sender.send(response).await.map_err(send_error)?;
                    }
                };
            }
            (GameRequest::CreateRoom { settings }, user) => {
                let response = match user.role {
                    Role::Guest => {
                        let settings = match settings {
                            None => server.default_settings,
                            Some(settings) => settings,
                        };

                        let room = Room::create(user.id, settings);
                        let room_id = *room.id();
                        user.role = Role::Host { room_id };
                        server.rooms.insert(
                            room_id,
                            RoomAgent {
                                room,
                                host: user.sender.clone(),
                                contestant: None,
                            },
                        );
                        GameResponse::RoomCreated {
                            id: room_id,
                            settings,
                        }
                    }
                    _ => GameResponse::GameError {
                        cause: Error::InvalidOperation,
                    },
                };
                user.sender.send(response).await.map_err(send_error)?;
            }
            (request, user) => {
                match &user.role {
                    Role::Contestant { room_id } | Role::Host { room_id } => {
                        match server.rooms.get_mut(room_id) {
                            Some(mut ra) => {
                                let room = &mut ra.room;
                                match request {
                                    GameRequest::ExitRoom { id } => {
                                        if id != *room.id() {
                                            eprintln!("exit room error: {} != {}", id, room.id())
                                        }
                                        ra.publish(GameResponse::Exited)
                                            .await
                                            .map_err(send_error)?;
                                        break;
                                    }
                                    GameRequest::Ready { ready } => {
                                        let response = room
                                            .contestant_ready(ready)
                                            .map(|_| GameResponse::Ready { ready })?; // 失败了不应该返回
                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    GameRequest::Choose { chosen } => {
                                        let response = match chosen {
                                            Index::Random => room.choose_random().map(|chosen| {
                                                GameResponse::Chosen {
                                                    chosen,
                                                    random: true,
                                                }
                                            }),
                                            Index::Specified(chosen) => {
                                                room.choose(chosen).map(|_| GameResponse::Chosen {
                                                    chosen,
                                                    random: false,
                                                })
                                            }
                                        }?;
                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    GameRequest::Decide { decision } => {
                                        let response = room
                                            .decide(decision)
                                            .map(|result| GameResponse::Decided { result })?;
                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    GameRequest::UpdateSettings { settings } => {
                                        let notify = room.update_settings(settings)?;
                                        let response =
                                            GameResponse::SettingsUpdated { settings, notify };

                                        if notify {
                                            ra.publish(response).await.map_err(send_error)?;
                                        } else {
                                            user.sender.send(response).await.map_err(send_error)?;
                                        }
                                    }
                                    GameRequest::Start { prize } => {
                                        let (host_resp, contestant_resp) = match prize {
                                            Index::Random => room.start_random().map(|prize| {
                                                (
                                                    GameResponse::Started {
                                                        prize,
                                                        random: true,
                                                    },
                                                    GameResponse::ContestantStarted {
                                                        random: true,
                                                    },
                                                )
                                            }),
                                            Index::Specified(prize) => {
                                                room.start(prize).map(|_| {
                                                    (
                                                        GameResponse::Started {
                                                            prize,
                                                            random: false,
                                                        },
                                                        GameResponse::ContestantStarted {
                                                            random: false,
                                                        },
                                                    )
                                                })
                                            }
                                        }?;

                                        ra.host.send(host_resp).await.map_err(send_error)?;
                                        match &ra.contestant {
                                            None => {}
                                            Some(contestant) => {
                                                contestant
                                                    .send(contestant_resp)
                                                    .await
                                                    .map_err(send_error)?;
                                            }
                                        }
                                    }
                                    GameRequest::Reveal { left } => {
                                        let response = match left {
                                            Index::Random => room.reveal_random().map(|left| {
                                                GameResponse::Revealed { left, random: true }
                                            }),
                                            Index::Specified(left) => {
                                                room.reveal(left).map(|_| GameResponse::Revealed {
                                                    left,
                                                    random: false,
                                                })
                                            }
                                        }?;

                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    GameRequest::Complete { kick_contestant } => {
                                        let response =
                                            room.complete(kick_contestant).map(|results| {
                                                let result = GameResult::calculate(
                                                    room.settings().doors,
                                                    results,
                                                );
                                                GameResponse::Completed { result }
                                            })?;
                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    _req => {}
                                }
                            }
                            None => {
                                let response = GameResponse::ServerError {
                                    cause: ServerError::RoomNotFound { id: *room_id },
                                };
                                user.sender.send(response).await.map_err(send_error)?;
                            }
                        }
                    }
                    _ => {
                        let response = GameResponse::GameError {
                            cause: Error::InvalidOperation,
                        };
                        user.sender.send(response).await.map_err(send_error)?;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn websocket_loop(
    mut socket: WebSocket,
    req_sender: Sender<GameRequest>,
    mut resp_receiver: Receiver<GameResponse>,
) -> anyhow::Result<()> {
    // 监听 socket 以及 room 中其他成员广播的消息
    loop {
        tokio::select! {
            option = socket.recv() => {
                if let Some(result) = option {
                    let message = result?;
                    match message {
                        Message::Text(request) => {
                            let request: GameRequest = serde_json::from_str(&request)?;
                            req_sender.send(request).await.map_err(send_error)?;
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
                    eprintln!("Connection closed");
                    break;
                }
            }
            resp = resp_receiver.recv() => {
                match resp {
                    Some(response) => {
                        socket.send(Message::Text(serde_json::to_string(&response)?)).await?;
                    }
                    None => {
                        eprintln!("Response channel closed");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

fn send_error<T>(_: T) -> anyhow::Error {
    anyhow::anyhow!("Failed to send message: channel closed.")
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

#[derive(thiserror::Error, Debug, Serialize, Deserialize, Copy, Clone)]
enum ServerError {
    #[error("Room not found: {}", .id)]
    RoomNotFound { id: Uuid },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "resp")]
enum GameResponse {
    UserCreated {
        id: Uuid,
    },
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
