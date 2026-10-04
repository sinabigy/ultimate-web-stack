//! Realtime delivery: Server-Sent Events (default; HTTP/2 friendly, auto-reconnect, works
//! through most proxies) and WebSocket (bidirectional). Both read from the event bus and end
//! when the server shuts down. Delivery is filtered per subscriber by `filter_for`, which is
//! the single place that decides who may see an event.

use std::{convert::Infallible, time::Duration};

use app_domain::{RealtimeEvent, event::Audience};
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::{Stream, StreamExt};
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

/// Who is listening. Built by the auth layer from *server-verified* memberships.
#[derive(Debug, Clone, Default)]
pub struct Subscriber {
    pub user_id: Option<uuid::Uuid>,
    pub organizations: Vec<uuid::Uuid>,
    pub system_admin: bool,
}

/// Deny by default: an event is delivered only if its audience explicitly includes the subscriber.
pub fn filter_for(sub: &Subscriber, ev: &RealtimeEvent) -> bool {
    match ev.audience() {
        Audience::Everyone => true,
        Audience::Organization(org) => sub.organizations.contains(&org),
        Audience::User(u) => sub.user_id == Some(u),
        Audience::SystemAdmins => sub.system_admin,
    }
}

/// Stream of events for one subscriber; ends on shutdown. Lagged receivers skip ahead.
pub fn event_stream(state: &AppState, sub: Subscriber) -> impl Stream<Item = RealtimeEvent> + Send + use<> {
    let rx = state.events.subscribe();
    let shutdown = state.lifecycle.shutdown.clone();
    futures::stream::unfold((rx, shutdown, sub), |(mut rx, shutdown, sub)| async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return None,
                msg = rx.recv() => match msg {
                    Ok(ev) if filter_for(&sub, &ev) => return Some((ev, (rx, shutdown, sub))),
                    Ok(_) => {}
                    Err(RecvError::Lagged(n)) => {
                        metrics::counter!("app_realtime_lagged_total").increment(n);
                    }
                    Err(RecvError::Closed) => return None,
                }
            }
        }
    })
}

pub fn sse_response(state: &AppState, sub: Subscriber) -> Sse<impl Stream<Item = Result<Event, Infallible>> + use<>> {
    let stream = event_stream(state, sub).map(|ev| {
        Ok(Event::default()
            .event(ev.kind())
            .json_data(&ev)
            .unwrap_or_else(|_| Event::default().comment("serialization error")))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// Public demo stream (heartbeats and `Everyone` events only). Authenticated streams live
/// under `/api/v1/events` and pass a real `Subscriber`.
pub async fn public_sse(State(state): State<AppState>) -> impl IntoResponse {
    sse_response(&state, Subscriber::default())
}

pub fn ws_response(state: AppState, ws: WebSocketUpgrade, sub: Subscriber) -> Response {
    ws.max_message_size(64 * 1024).on_upgrade(move |socket| ws_loop(state, socket, sub))
}

async fn ws_loop(state: AppState, mut socket: WebSocket, sub: Subscriber) {
    let mut events = Box::pin(event_stream(&state, sub));
    metrics::gauge!("app_ws_connections").increment(1.0);
    loop {
        tokio::select! {
            ev = events.next() => match ev {
                Some(ev) => {
                    let Ok(text) = serde_json::to_string(&ev) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() { break; }
                }
                None => {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            },
            incoming = socket.recv() => match incoming {
                // Clients may send pings or app messages; this example only needs keep-alive.
                Some(Ok(Message::Ping(p))) => { let _ = socket.send(Message::Pong(p)).await; }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    metrics::gauge!("app_ws_connections").decrement(1.0);
}

pub async fn public_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws_response(state, ws, Subscriber::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn delivery_is_deny_by_default() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let member_a = Subscriber { user_id: Some(Uuid::now_v7()), organizations: vec![a], system_admin: false };
        let ev_b = RealtimeEvent::RunCreated { run_id: Uuid::now_v7(), organization_id: b, requested: 1 };
        let ev_a = RealtimeEvent::RunCreated { run_id: Uuid::now_v7(), organization_id: a, requested: 1 };
        assert!(filter_for(&member_a, &ev_a));
        assert!(!filter_for(&member_a, &ev_b), "cross-tenant event must not be delivered");
        let health =
            RealtimeEvent::ProviderHealth { provider: "p".into(), concurrency_limit: 1, inflight: 0, healthy: true };
        assert!(!filter_for(&member_a, &health), "system events only for system admins");
        let note =
            RealtimeEvent::Notification { user_id: Uuid::now_v7(), notification_id: Uuid::now_v7(), title: "t".into() };
        assert!(!filter_for(&member_a, &note), "other users' notifications are private");
        assert!(!filter_for(&Subscriber::default(), &ev_a), "anonymous sees no tenant data");
    }
}
