use std::{
    net::IpAddr,
    sync::RwLock,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use api::{BlockingPause, BlockingScope, PauseBlockingRequest};

#[derive(Default)]
pub struct Pauses(RwLock<Option<ActivePause>>);

struct ActivePause {
    status: BlockingPause,
    deadline: Instant,
}

impl Pauses {
    pub fn set(&self, mut request: PauseBlockingRequest) -> Result<BlockingPause, &'static str> {
        if !(1..=86_400).contains(&request.duration_seconds) {
            return Err("Choose a duration between 1 second and 24 hours.");
        }
        if let BlockingScope::Clients(clients) = &mut request.target {
            if clients.is_empty() || clients.len() > 1024 {
                return Err("Choose between 1 and 1024 device addresses.");
            }
            for client in clients.iter_mut() {
                *client = client.to_canonical();
            }
            clients.sort_unstable();
            clients.dedup();
        }
        let duration = Duration::from_secs(request.duration_seconds);
        let status = BlockingPause {
            target: request.target,
            until_ms: (SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                + duration)
                .as_millis() as u64,
        };
        *self.0.write().expect("pause lock poisoned") = Some(ActivePause {
            status: status.clone(),
            deadline: Instant::now() + duration,
        });
        Ok(status)
    }

    pub fn clear(&self) {
        *self.0.write().expect("pause lock poisoned") = None;
    }

    pub fn status(&self) -> Option<BlockingPause> {
        self.status_at(Instant::now())
    }

    fn status_at(&self, now: Instant) -> Option<BlockingPause> {
        self.0
            .read()
            .expect("pause lock poisoned")
            .as_ref()
            .filter(|pause| now < pause.deadline)
            .map(|pause| pause.status.clone())
    }

    pub fn includes(&self, client: IpAddr) -> bool {
        let guard = self.0.read().expect("pause lock poisoned");
        let Some(pause) = guard
            .as_ref()
            .filter(|pause| Instant::now() < pause.deadline)
        else {
            return false;
        };
        match &pause.status.target {
            BlockingScope::All => true,
            BlockingScope::Clients(clients) => clients.contains(&client.to_canonical()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_clients_are_canonicalized_and_other_clients_stay_blocked() {
        let pauses = Pauses::default();
        pauses
            .set(PauseBlockingRequest {
                duration_seconds: 60,
                target: BlockingScope::Clients(vec!["::ffff:192.168.1.2".parse().unwrap()]),
            })
            .unwrap();
        assert!(pauses.includes("192.168.1.2".parse().unwrap()));
        assert!(!pauses.includes("192.168.1.3".parse().unwrap()));
        assert!(
            pauses
                .status_at(Instant::now() + Duration::from_secs(60))
                .is_none()
        );
        pauses.clear();
        assert!(!pauses.includes("192.168.1.2".parse().unwrap()));
    }

    #[test]
    fn global_pause_replaces_selection_and_invalid_requests_leave_it_unchanged() {
        let pauses = Pauses::default();
        pauses
            .set(PauseBlockingRequest {
                duration_seconds: 60,
                target: BlockingScope::Clients(vec!["192.168.1.2".parse().unwrap()]),
            })
            .unwrap();
        assert!(!pauses.includes("2001:db8::1".parse().unwrap()));
        let request = PauseBlockingRequest {
            duration_seconds: 60,
            target: BlockingScope::All,
        };
        pauses.set(request.clone()).unwrap();
        assert!(pauses.includes("2001:db8::1".parse().unwrap()));
        for duration_seconds in [0, 86_401, u64::MAX] {
            assert!(
                pauses
                    .set(PauseBlockingRequest {
                        duration_seconds,
                        ..request.clone()
                    })
                    .is_err()
            );
        }
        assert!(
            pauses
                .set(PauseBlockingRequest {
                    duration_seconds: 60,
                    target: BlockingScope::Clients(vec![])
                })
                .is_err()
        );
        assert_eq!(pauses.status().unwrap().target, BlockingScope::All);
    }
}
