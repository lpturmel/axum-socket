//! # Axum Socket
//!
//! Axum Socket is a library that provides a simple way to handle websockets in the axum web framework.
//!
//! Define axum-like routes for websockets to be handled by your application.
//!
use axum::extract::ws::Message;
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures::channel::mpsc::UnboundedSender;
use futures::future::BoxFuture;
use futures::stream::StreamExt;
use futures::{Future, FutureExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

async fn runner(
    ws: WebSocketUpgrade,
    State(dispatcher): State<EventDispatcher>,
) -> impl IntoResponse {
    let dispatcher = dispatcher.clone();
    ws.on_upgrade(|socket| async move {
        let (user_tx, mut user_rx) = socket.split();
        let (tx, rx) = futures::channel::mpsc::unbounded();

        tokio::task::spawn(rx.forward(user_tx).map(|result| {
            if let Err(e) = result {
                eprintln!("websocket send error: {}", e);
            }
        }));
        let id = uuid::Uuid::new_v4().to_string();

        dispatcher.add_socket(&id, tx);

        if let Some(on_connect) = dispatcher.run_connect() {
            on_connect(id.clone(), dispatcher.clone()).await;
        }

        while let Some(result) = user_rx.next().await {
            let msg = match result {
                Ok(msg) => msg,
                Err(e) => {
                    eprintln!("error: {}", e);
                    break;
                }
            };
            match msg {
                Message::Text(text) => {
                    let event: WsEvent<serde_json::Value> = serde_json::from_str(&text).unwrap();
                    let event_slug = event.slug.clone();
                    let listeners = dispatcher.listeners();
                    for (slug, listener) in listeners {
                        if event_slug == slug {
                            listener(event.clone(), id.clone(), dispatcher.clone()).await;
                        }
                    }
                }
                Message::Binary(_) => {
                    eprintln!("binary messages are not supported");
                    break;
                }
                Message::Close(_) => {
                    break;
                }
                _ => {}
            }
        }
        // Clean up
        dispatcher.remove_socket(&id);

        if let Some(on_disconnect) = dispatcher.run_disconnect() {
            on_disconnect(id.clone(), dispatcher.clone()).await;
        }
    })
}

/// The entry point for the websocket service
///
/// This method takes a route for the websocket service, an `EventDispatcher` and an optional `Router` for routes that might need access to the WS state
///
/// # Example
///
/// ```no_run
/// use axum_socket::{ws_service, EventDispatcher};
/// use axum::{extract::ws::Message, Router};
///
/// async fn on_connect(socket_id: String, state: EventDispatcher) {
///    println!("Client {} connected", socket_id);
/// }
/// #[tokio::main]
/// async fn main() {
///     let mut event = EventDispatcher::new();
///
///     let app = Router::new().nest("/", ws_service("/ws", event, None));
///
///     let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
///     axum::serve(listener, app).await.unwrap();
/// }
pub fn ws_service(
    ws_route: &str,
    dispatcher: EventDispatcher,
    http_router: Option<Router<EventDispatcher>>,
) -> Router {
    Router::new()
        .route(ws_route, get(runner))
        .nest("/", http_router.unwrap_or_default())
        .with_state(dispatcher)
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WsEvent<T> {
    /// The event slug to match on
    slug: String,
    #[serde(flatten)]
    data: T,
}

type Sender = UnboundedSender<Result<Message, axum::Error>>;

type ListenerClosure = Arc<
    dyn Fn(WsEvent<serde_json::Value>, String, EventDispatcher) -> BoxFuture<'static, ()>
        + Send
        + Sync
        + 'static,
>;
type OnConnectClosure =
    Arc<dyn Fn(String, EventDispatcher) -> BoxFuture<'static, ()> + Send + Sync + 'static>;
type OnDisconnectClosure =
    Arc<dyn Fn(String, EventDispatcher) -> BoxFuture<'static, ()> + Send + Sync + 'static>;

#[derive(Clone, Default)]
pub struct EventDispatcher {
    listeners: Arc<Mutex<Vec<(String, ListenerClosure)>>>,
    sockets: Arc<Mutex<HashMap<String, Sender>>>,
    on_connect: Option<OnConnectClosure>,
    on_disconnect: Option<OnDisconnectClosure>,
}

impl EventDispatcher {
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(Mutex::new(Vec::new())),
            sockets: Arc::new(Mutex::new(HashMap::new())),
            on_connect: None,
            on_disconnect: None,
            // socket: Arc::new(ws),
        }
    }

    /// Register a closure to be called when a new client connects
    ///
    /// # Example
    ///
    /// ```no_run
    /// use axum_socket::{ws_service, EventDispatcher};
    /// use axum::{extract::ws::Message, Router};
    ///
    /// async fn on_connect(socket_id: String, state: EventDispatcher) {
    ///    println!("Client {} connected", socket_id);
    /// }
    /// #[tokio::main]
    /// async fn main() {
    ///     let mut event = EventDispatcher::new().on_connect(on_connect);
    ///
    ///     let app = Router::new().nest("/", ws_service("/ws", event, None));
    ///
    ///     let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    ///     axum::serve(listener, app).await.unwrap();
    /// }
    pub fn on_connect<F, Fut>(mut self, listener: F) -> Self
    where
        F: Fn(String, EventDispatcher) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        assert!(self.on_connect.is_none(), "on_connect already set");
        let listener = Arc::new(
            move |socket_id: String, dispatcher: EventDispatcher| -> BoxFuture<'static, ()> {
                Box::pin(listener(socket_id, dispatcher))
            },
        );
        self.on_connect = Some(listener);
        self
    }

    /// Runs the on_connect closure
    fn run_connect(&self) -> Option<OnConnectClosure> {
        self.on_connect.clone()
    }

    /// Register a closure to be called when a new client disconnects
    ///
    /// # Example
    ///
    /// ```no_run
    /// use axum_socket::{ws_service, EventDispatcher};
    /// use axum::{extract::ws::Message, Router};
    ///
    /// async fn on_disconnect(socket_id: String, state: EventDispatcher) {
    ///    println!("Client {} disconnected!", socket_id);
    /// }
    /// #[tokio::main]
    /// async fn main() {
    ///     let mut event = EventDispatcher::new().on_disconnect(on_disconnect);
    ///
    ///     let app = Router::new().nest("/", ws_service("/ws", event, None));
    ///
    ///     let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    ///     axum::serve(listener, app).await.unwrap();
    /// }
    pub fn on_disconnect<F, Fut>(mut self, listener: F) -> Self
    where
        F: Fn(String, EventDispatcher) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        assert!(self.on_disconnect.is_none(), "on_disconnect already set");
        let listener = Arc::new(
            move |socket_id: String, dispatcher: EventDispatcher| -> BoxFuture<'static, ()> {
                Box::pin(listener(socket_id, dispatcher))
            },
        );
        self.on_disconnect = Some(listener);
        self
    }

    /// Runs the on_disconnect closure
    fn run_disconnect(&self) -> Option<OnDisconnectClosure> {
        self.on_disconnect.clone()
    }

    /// Get a single socket by id
    pub fn get_socket(&self, id: &str) -> Option<Sender> {
        let guard = self.sockets.lock().unwrap();
        guard.get(id).cloned()
    }

    /// Get all a vector of all sockets and their ids
    pub fn sockets(&self) -> Vec<(String, Sender)> {
        let guard = self.sockets.lock().unwrap();
        guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    fn add_socket(&self, id: &str, socket: Sender) {
        let mut sockets = self.sockets.lock().unwrap();
        sockets.insert(id.to_string(), socket);
    }

    fn remove_socket(&self, socket_id: &str) {
        let mut sockets = self.sockets.lock().unwrap();
        sockets.remove(socket_id);
    }

    /// Register an event listener
    ///
    /// The event will match on the `event_slug` provided which represents the slug JSON field of
    /// the event
    ///
    ///
    /// # Example
    ///     
    /// ```no_run
    /// use axum_socket::{ws_service, EventDispatcher};
    /// use axum::{extract::ws::Message, Router};
    ///
    /// async fn on_message(event: axum_socket::WsEvent<serde_json::Value>, socket_id: String, state: axum_socket::EventDispatcher) {
    ///    println!("Received event: {:?}", event);
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mut event = EventDispatcher::new().on("message", on_message);
    ///
    ///     let app = Router::new().nest("/", ws_service("/ws", event, None));
    ///
    ///     let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    ///     axum::serve(listener, app).await.unwrap();
    /// }
    pub fn on<F, Fut>(self, event_slug: &str, listener: F) -> Self
    where
        F: Fn(WsEvent<serde_json::Value>, String, EventDispatcher) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = Arc::new(
            move |event: WsEvent<serde_json::Value>,
                  socket_id: String,
                  dispatcher: EventDispatcher|
                  -> BoxFuture<'static, ()> {
                Box::pin(listener(event, socket_id, dispatcher))
            },
        );
        {
            let mut listeners = self.listeners.lock().unwrap();
            listeners.push((event_slug.to_string(), listener));
        }
        self
    }

    pub fn listeners(&self) -> Vec<(String, ListenerClosure)> {
        self.listeners.lock().unwrap().iter().cloned().collect()
    }
}
