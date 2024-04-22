use axum::{extract::ws::Message, Router};
use axum_socket::{ws_service, EventDispatcher, WsEvent};

#[tokio::main]
async fn main() {
    let event = EventDispatcher::new()
        .on("ping", pong)
        .on_connect(on_connect)
        .on_disconnect(on_disconnect);

    let app = Router::new().nest("/", ws_service("/ws", event, None));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Listening on: {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

async fn on_connect(socket_id: String, state: EventDispatcher) {
    println!("Client {} connected", socket_id);
    let sockets = state.sockets();
    let my_socket = state.get_socket(&socket_id).unwrap();
    let _ = my_socket.unbounded_send(Ok(Message::Text(socket_id.clone())));

    let sockets = sockets.iter().filter(|(id, _)| id != &socket_id);
    let message = format!("Client {} connected", socket_id);
    for (_, socket) in sockets {
        let _ = socket.unbounded_send(Ok(Message::Text(serde_json::to_string(&message).unwrap())));
    }
}

async fn on_disconnect(socket_id: String, _: EventDispatcher) {
    println!("Client {} disconnected", socket_id);
    // clean up is handled by the library
}

async fn pong(_: WsEvent<serde_json::Value>, socket_id: String, state: EventDispatcher) {
    println!("Ponging...");
    let socket = state.get_socket(&socket_id).expect("socket not found");
    let _ = socket.unbounded_send(Ok(Message::Text(serde_json::to_string("pong").unwrap())));
}
