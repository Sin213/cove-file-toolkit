use dashmap::DashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub type JobId = String;

pub struct JobHandle {
    pub token: CancellationToken,
}

#[derive(Default, Clone)]
pub struct JobManager {
    jobs: Arc<DashMap<JobId, JobHandle>>,
}

impl JobManager {
    pub fn create(&self) -> (JobId, CancellationToken) {
        let id = uuid::Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        self.jobs.insert(
            id.clone(),
            JobHandle {
                token: token.clone(),
            },
        );
        (id, token)
    }

    pub fn cancel(&self, id: &str) -> bool {
        if let Some(handle) = self.jobs.get(id) {
            handle.token.cancel();
            true
        } else {
            false
        }
    }

    pub fn remove(&self, id: &str) {
        self.jobs.remove(id);
    }
}
