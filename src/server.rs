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
use tower_http::trace::TraceLayer;
use tracing::Level;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout.with_max_level(Level::INFO)),
        )
        .init();

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
        .layer(TraceLayer::new_for_http())
        .layer(Extension(Server::default()));

    tracing::info!(%addr, "Server started.");
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
            tracing::error!("Failed to send UserCreated response.");
            return;
        }

        tracing::info!(user = %user.id, "User created.");

        let s = server.clone();
        tokio::spawn(async move {
            if let Err(cause) = request_handler(user, s, req_receiver).await {
                tracing::error!(%cause, "Request handler error.");
            }
        });
        if let Err(cause) = websocket_loop(socket, req_sender, resp_receiver).await {
            tracing::error!(%cause, "Websocket loop error.");
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

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RoomInfo {
    id: Uuid,
    settings: Settings,
}

impl RoomInfo {
    pub fn new(id: Uuid, settings: Settings) -> Self {
        Self { id, settings }
    }
}

impl From<&Room> for RoomInfo {
    fn from(room: &Room) -> Self {
        RoomInfo::new(*room.id(), room.settings())
    }
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

#[derive(Debug, Copy, Clone)]
enum Role {
    Guest,
    Host { room_id: Uuid },
    Contestant { room_id: Uuid },
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

impl Drop for User {
    fn drop(&mut self) {
        tracing::info!(user = %self.id, "User disconnected.");
    }
}

#[derive(Debug)]
struct RoomDropper {
    rooms: Arc<DashMap<Uuid, RoomAgent>>,
    id: Option<Uuid>,
}

impl RoomDropper {
    pub fn new(rooms: Arc<DashMap<Uuid, RoomAgent>>) -> Self {
        Self { rooms, id: None }
    }

    pub fn set_room(&mut self, id: Uuid) {
        if let Some(room_id) = self.id {
            self.rooms.remove(&room_id);
        }
        self.id = Some(id);
    }
}

impl Drop for RoomDropper {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.rooms.remove(&id);
            tracing::warn!(room = %id, "Room dropped.")
        }
    }
}

#[tracing::instrument(skip_all, fields(user = %user.id, role = ?user.role))]
async fn request_handler(
    mut user: User,
    server: Server,
    mut receiver: Receiver<GameRequest>,
) -> anyhow::Result<()> {
    let mut room_dropper = RoomDropper::new(server.rooms.clone());

    while let Some(request) = receiver.recv().await {
        match (request, &mut user) {
            (GameRequest::ListRooms { page, size }, _) => {
                let total = server.rooms.len() as u32;
                let rooms = server
                    .rooms
                    .iter()
                    .skip((page * size) as usize)
                    .map(|ra| RoomInfo::from(&ra.room))
                    .collect();
                let response = GameResponse::RoomList {
                    rooms,
                    page,
                    size,
                    total,
                };
                tracing::info!(?response, "List rooms.");
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

                            let host_resp = GameResponse::RoomEntered {
                                contestant_id: user.id,
                            };

                            let contestant_resp = GameResponse::ContestantRoomEntered {
                                info: RoomInfo::from(&ra.room),
                            };

                            tracing::info!(?host_resp, "Enter rooms.");
                            ra.host.send(host_resp).await.map_err(send_error)?;
                            if let Some(contestant) = &ra.contestant {
                                contestant.send(contestant_resp).await.map_err(send_error)?;
                            }
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
                        let response = GameResponse::RoomCreated {
                            info: RoomInfo::from(&room),
                        };
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
                        room_dropper.set_room(room_id);
                        response
                    }
                    _ => GameResponse::GameError {
                        cause: Error::InvalidOperation,
                    },
                };

                tracing::info!(?response, "Create room.");
                user.sender.send(response).await.map_err(send_error)?;
            }
            (request, user) => {
                match user.role {
                    Role::Host { room_id } => {
                        let mut remove = false;

                        match server.rooms.get_mut(&room_id) {
                            Some(mut ra) => {
                                let room = &mut ra.room;
                                match request {
                                    GameRequest::ExitRoom { id } => {
                                        if id != *room.id() {
                                            tracing::error!(
                                                "exit room error: {} != {}.",
                                                id,
                                                room.id()
                                            )
                                        }

                                        user.role = Role::Guest;
                                        let response = GameResponse::Exited { user_id: user.id };
                                        tracing::info!(?response, "Host exit room.");
                                        ra.publish(response).await.map_err(send_error)?;
                                        remove = true;
                                    }
                                    GameRequest::UpdateSettings { settings } => {
                                        let result = room.update_settings(settings).map(|notify| {
                                            (
                                                GameResponse::SettingsUpdated { settings, notify },
                                                notify,
                                            )
                                        });

                                        match result {
                                            Ok((response, notify)) => {
                                                tracing::info!(?response, %notify, "Update settings.");
                                                if notify {
                                                    ra.publish(response)
                                                        .await
                                                        .map_err(send_error)?;
                                                } else {
                                                    user.sender
                                                        .send(response)
                                                        .await
                                                        .map_err(send_error)?;
                                                }
                                            }
                                            Err(cause) => {
                                                user.sender
                                                    .send(GameResponse::GameError { cause })
                                                    .await
                                                    .map_err(send_error)?;
                                            }
                                        }
                                    }
                                    GameRequest::Start { prize } => {
                                        let result = match prize {
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
                                        };

                                        match result {
                                            Ok((host_resp, contestant_resp)) => {
                                                tracing::info!(
                                                    ?host_resp,
                                                    ?contestant_resp,
                                                    "Start."
                                                );

                                                ra.host
                                                    .send(host_resp)
                                                    .await
                                                    .map_err(send_error)?;
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
                                            Err(cause) => {
                                                ra.host
                                                    .send(GameResponse::GameError { cause })
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
                                        }
                                        .into();

                                        tracing::info!(?response, "Reveal.");
                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    GameRequest::Complete { kick_contestant } => {
                                        let response = room
                                            .complete(kick_contestant)
                                            .map(|results| {
                                                let result = GameResult::calculate(
                                                    room.settings().doors,
                                                    results,
                                                );
                                                GameResponse::Completed { result }
                                            })
                                            .into();
                                        tracing::info!(?response, %kick_contestant, "Complete.");
                                        ra.publish(response).await.map_err(send_error)?;
                                        if kick_contestant {
                                            ra.contestant = None;
                                        }
                                    }
                                    request => {
                                        let response = GameResponse::GameError {
                                            cause: Error::InvalidOperation,
                                        };
                                        tracing::warn!(?request, ?user.role, "Invalid operation.");
                                        user.sender.send(response).await.map_err(send_error)?;
                                    }
                                }
                            }
                            None => {
                                let response = GameResponse::ServerError {
                                    cause: ServerError::RoomNotFound { id: room_id },
                                };
                                user.sender.send(response).await.map_err(send_error)?;

                                tracing::error!(user = %user.id, "Room not found, user role changed to guest.");
                                user.role = Role::Guest;
                                continue;
                            }
                        }

                        if remove {
                            // 这个删除不能在 get_mut 之后的上下文进行，会导致死锁
                            server.rooms.remove(&room_id);
                        }
                    }
                    Role::Contestant { room_id } => {
                        match server.rooms.get_mut(&room_id) {
                            Some(mut ra) => {
                                let room = &mut ra.room;
                                if matches!(room.state(), RoomState::Created) {
                                    tracing::error!(user = %user.id, room = %room_id, "User may be kicked out of room.");
                                    user.role = Role::Guest;
                                    user.sender
                                        .send(GameResponse::Exited { user_id: user.id })
                                        .await
                                        .map_err(send_error)?;
                                    continue;
                                }
                                match request {
                                    GameRequest::ExitRoom { id } => {
                                        if id != *room.id() {
                                            tracing::error!(
                                                "exit room error: {} != {}.",
                                                id,
                                                room.id()
                                            )
                                        }

                                        // infallible
                                        room.kick_contestant().unwrap_or_default();

                                        user.role = Role::Guest;
                                        let response = GameResponse::Exited { user_id: user.id };
                                        tracing::info!(?response, "Contestant exit room.");
                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    GameRequest::Ready { ready } => {
                                        let response = room
                                            .contestant_ready(ready)
                                            .map(|_| GameResponse::Ready { ready })
                                            .into();

                                        tracing::info!(?ready, "Ready.");
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
                                        }
                                        .into();
                                        tracing::info!(?response, "Choose.");
                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    GameRequest::Decide { decision } => {
                                        let response = room
                                            .decide(decision)
                                            .map(|result| GameResponse::Decided { result })
                                            .into();
                                        tracing::info!(?response, "Decide.");
                                        ra.publish(response).await.map_err(send_error)?;
                                    }
                                    request => {
                                        let response = GameResponse::GameError {
                                            cause: Error::InvalidOperation,
                                        };
                                        tracing::warn!(?request, ?user.role, "Invalid operation.");
                                        user.sender.send(response).await.map_err(send_error)?;
                                    }
                                }
                            }
                            None => {
                                let response = GameResponse::ServerError {
                                    cause: ServerError::RoomNotFound { id: room_id },
                                };
                                tracing::warn!(%room_id, "Room not found.");
                                user.sender.send(response).await.map_err(send_error)?;

                                tracing::error!(user = %user.id, "Room not found, user role changed to guest.");
                                user.role = Role::Guest;
                                continue;
                            }
                        }
                    }
                    role => {
                        let response = GameResponse::GameError {
                            cause: Error::InvalidOperation,
                        };
                        tracing::warn!(?request, ?role, "Invalid operation.");
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
                                tracing::info!(
                                    "Connection closed: code = {}, reason = {}.",
                                    c.code, c.reason
                                );
                                break;
                            }
                            None => {
                                tracing::info!("Connection closed without close frame.",);
                                break;
                            }
                        },
                        _ => {}
                    }
                } else {
                    tracing::error!("Connection closed.");
                    break;
                }
            }
            resp = resp_receiver.recv() => {
                match resp {
                    Some(response) => {
                        socket.send(Message::Text(serde_json::to_string(&response)?)).await?;
                    }
                    None => {
                        tracing::error!("Response channel closed.");
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
        rooms: Vec<RoomInfo>,
        page: u32,
        size: u32,
        total: u32,
    },
    RoomCreated {
        info: RoomInfo,
    },
    Exited {
        user_id: Uuid,
    },
    RoomEntered {
        contestant_id: Uuid,
    },
    ContestantRoomEntered {
        info: RoomInfo,
    },
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

impl From<Result<GameResponse>> for GameResponse {
    fn from(result: Result<GameResponse>) -> Self {
        match result {
            Ok(response) => response,
            Err(cause) => GameResponse::GameError { cause },
        }
    }
}

impl From<std::result::Result<GameResponse, ServerError>> for GameResponse {
    fn from(result: std::result::Result<GameResponse, ServerError>) -> Self {
        match result {
            Ok(response) => response,
            Err(cause) => GameResponse::ServerError { cause },
        }
    }
}
