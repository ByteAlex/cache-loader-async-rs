use tokio::task::JoinHandle;
use std::hash::Hash;
use futures::Future;
use crate::internal_cache::{CacheAction, InternalCacheStore, CacheMessage};

#[derive(Debug, Clone)]
pub struct CacheLoadingError {
    pub reason_phrase: String,
    // todo: nested errors
}

#[derive(Debug)]
pub enum CacheResult<V> {
    Found(V),
    Loading(JoinHandle<Result<V, CacheLoadingError>>),
    None,
}

pub struct CacheHandle(JoinHandle<()>);

#[derive(Debug, Clone)]
pub struct LoadingCache<K, V> {
    tx: tokio::sync::mpsc::Sender<CacheMessage<K, V>>
}

impl<
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Sized + Send + 'static,
> LoadingCache<K, V> {
    pub fn new<T, F>(loader: T) -> (LoadingCache<K, V, >, CacheHandle)
        where F: Future<Output=Option<V>> + Sized + Send + 'static,
              T: Fn(K) -> F + Send + 'static {
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let store = InternalCacheStore::new(tx.clone(), loader);
        let handle = store.run(rx);
        (LoadingCache {
            tx
        }, CacheHandle(handle))
    }

    pub async fn get(&self, key: K) -> Result<V, CacheLoadingError> {
        self.send_cache_action(CacheAction::Get(key)).await
            .map(|opt_result| opt_result.expect("Get should always return either V or CacheLoadingError"))
    }

    pub async fn set(&self, key: K, value: V) -> Result<Option<V>, CacheLoadingError> {
        self.send_cache_action(CacheAction::Set(key, value)).await
    }

    pub async fn update<U>(&self, key: K, update_fn: U) -> Result<V, CacheLoadingError>
        where U: FnOnce(V) -> V + Send + 'static {
        self.send_cache_action(CacheAction::Update(key, Box::new(update_fn))).await
            .map(|opt_result| opt_result.expect("Get should always return either V or CacheLoadingError"))
    }

    async fn send_cache_action(&self, action: CacheAction<K, V>) -> Result<Option<V>, CacheLoadingError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        match self.tx.send(CacheMessage {
            action,
            response: tx,
        }).await {
            Ok(_) => {
                match rx.await {
                    Ok(result) => {
                        match result {
                            CacheResult::Found(value) => { Ok(Some(value)) }
                            CacheResult::Loading(handle) => {
                                match handle.await {
                                    Ok(load_result) => {
                                        load_result.map(|v| Some(v))
                                    }
                                    Err(_) => {
                                        Err(CacheLoadingError { reason_phrase: "Error when trying to join loader future".to_owned() })
                                    }
                                }
                            }
                            CacheResult::None => { Ok(None) }
                        }
                    }
                    Err(_) => {
                        Err(CacheLoadingError { reason_phrase: "Error when receiving cache response".to_owned() })
                    }
                }
            }
            Err(_) => {
                Err(CacheLoadingError { reason_phrase: "Error when trying to submit cache request".to_owned() })
            }
        }
    }
}