//! Fake external effects use task egress; model transport stays on host loopback.
use super::{ProviderState, effect_state, external_effect};
use axum::{Router, routing::post};
use std::{
    net::{IpAddr, TcpListener, UdpSocket},
    sync::{Arc, Mutex},
    thread,
};
use tokio::sync::oneshot;

pub struct EffectService {
    pub url: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<thread::JoinHandle<()>>,
}

impl EffectService {
    pub(super) fn new(shared: Arc<Mutex<ProviderState>>) -> Self {
        // UDP connect selects a local interface without sending a packet, as in
        // the sandbox network-boundary regression. Do not forward host loopback.
        let route = UdpSocket::bind("0.0.0.0:0").unwrap();
        route.connect("192.0.2.1:9").unwrap();
        let IpAddr::V4(address) = route.local_addr().unwrap().ip() else {
            panic!("IPv4 host is required for task-egress fixtures")
        };
        assert!(!address.is_loopback());
        let listener = TcpListener::bind((address, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let app = Router::new()
                        .route("/effects", post(external_effect).get(effect_state))
                        .with_state(shared);
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            let _ = stopped.await;
                        })
                        .await
                        .unwrap();
                });
        });
        Self {
            url: format!("http://{address}"),
            stop: Some(stop),
            task: Some(task),
        }
    }
}

impl Drop for EffectService {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}
