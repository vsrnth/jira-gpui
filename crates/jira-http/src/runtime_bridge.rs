use std::sync::mpsc;

use jira_application::{ApplicationError, ErrorKind};
use tokio::{runtime::Builder, sync::oneshot};

type RuntimeJob = Box<dyn FnOnce(&tokio::runtime::Runtime) + Send + 'static>;

pub(super) struct RuntimeBridge {
    sender: mpsc::Sender<RuntimeJob>,
}

impl RuntimeBridge {
    pub(super) fn new() -> Result<Self, ()> {
        let (sender, receiver) = mpsc::channel::<RuntimeJob>();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("jira-http-runtime".to_owned())
            .spawn(move || {
                let runtime = match Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => {
                        let _ = startup_sender.send(Ok(()));
                        runtime
                    }
                    Err(_) => {
                        let _ = startup_sender.send(Err(()));
                        return;
                    }
                };
                while let Ok(job) = receiver.recv() {
                    job(&runtime);
                }
            })
            .map_err(|_| ())?;
        startup_receiver.recv().map_err(|_| ())??;
        Ok(Self { sender })
    }

    pub(super) async fn dispatch<T, F>(
        &self,
        operation: F,
    ) -> Result<Result<T, ApplicationError>, ApplicationError>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, ApplicationError>> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Box::new(move |runtime| {
                let result = runtime.block_on(operation);
                let _ = sender.send(result);
            }))
            .map_err(|_| {
                ApplicationError::new(ErrorKind::Internal, "Jira runtime is unavailable")
            })?;
        receiver
            .await
            .map_err(|_| ApplicationError::new(ErrorKind::Internal, "Jira runtime stopped"))
    }
}
