use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use super::SessionError;

type Responder = Box<dyn FnOnce(bool) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Pedidos de permissão em aberto de uma sessão. A tarefa que lê o agente registra *como*
/// responder cada pedido (capturando o peer e o id do protocolo); `respond_approval`
/// só precisa chamar `resolve`.
#[derive(Clone, Default)]
pub struct PendingApprovals(Arc<Mutex<HashMap<String, Responder>>>);

impl PendingApprovals {
    pub fn register<F, Fut>(&self, id: String, respond: F)
    where
        F: FnOnce(bool) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let responder: Responder = Box::new(move |approved| Box::pin(respond(approved)));
        self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(id, responder);
    }

    pub async fn resolve(&self, id: &str, approved: bool) -> Result<(), SessionError> {
        let responder = self.0.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
        match responder {
            Some(respond) => {
                respond(approved).await;
                Ok(())
            }
            None => Err(SessionError::NotReady(format!("pedido de aprovação desconhecido: {id}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resolve_runs_the_responder_once() {
        let pending = PendingApprovals::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        pending.register("a".into(), move |approved| async move {
            let _ = tx.send(approved);
        });
        pending.resolve("a", true).await.unwrap();
        assert_eq!(rx.recv().await, Some(true));
        assert!(pending.resolve("a", true).await.is_err());
        assert!(pending.resolve("nope", false).await.is_err());
    }
}
