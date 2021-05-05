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

pub type CacheHandle = JoinHandle<()>;

#[derive(Debug, Clone)]
pub struct LoadingCache<K, V> {
    tx: tokio::sync::mpsc::Sender<CacheMessage<K, V>>
}

impl<
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Sized + Send + 'static,
> LoadingCache<K, V> {
    /// Creates a new instance of a LoadingCache
    ///
    /// # Arguments
    ///
    /// * `loader` - A function which returns a Future<Output=Option<V>>
    ///
    /// # Return Value
    ///
    /// This method returns a tuple, with:
    /// 0 - The instance of the LoadingCache
    /// 1 - The CacheHandle which is a JoinHandle<()> and represents the task which operates
    ///     the cache
    ///
    /// # Examples
    ///
    /// ```
    /// async fn example() {
    ///     use cache_loader_async::cache_api::LoadingCache;
    ///     use std::collections::HashMap;
    ///     let static_db: HashMap<String, u32> =
    ///         vec![("foo".into(), 32), ("bar".into(), 64)]
    ///             .into_iter()
    ///             .collect();
    ///
    ///     let (cache, _) = LoadingCache::new(move |key: String| {
    ///         let db_clone = static_db.clone();
    ///         async move {
    ///             db_clone.get(&key).cloned()
    ///         }
    ///     });
    ///
    ///     let result = cache.get("foo".to_owned()).await.unwrap();
    ///
    ///     assert_eq!(result, 32);
    /// }
    /// ```
    pub fn new<T, F>(loader: T) -> (LoadingCache<K, V, >, CacheHandle)
        where F: Future<Output=Option<V>> + Sized + Send + 'static,
              T: Fn(K) -> F + Send + 'static {
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let store = InternalCacheStore::new(tx.clone(), loader);
        let handle = store.run(rx);
        (LoadingCache {
            tx
        }, handle)
    }

    /// Retrieves or loads the value for specified key from either cache or loader function
    ///
    /// # Arguments
    ///
    /// * `key` - The key which should be loaded
    ///
    /// # Return Value
    ///
    /// Returns a Result with:
    /// Ok - Value of type V
    /// Err - Error of type CacheLoadingError
    pub async fn get(&self, key: K) -> Result<V, CacheLoadingError> {
        self.send_cache_action(CacheAction::Get(key)).await
            .map(|opt_result| opt_result.expect("Get should always return either V or CacheLoadingError"))
    }

    /// Sets the value for specified key and bypasses eventual currently ongoing loads
    /// If a key has been set programmatically, eventual concurrent loads will not change
    /// the value of the key.
    ///
    /// # Arguments
    ///
    /// * `key` - The key which should be loaded
    ///
    /// # Return Value
    ///
    /// Returns a Result with:
    /// Ok - Previous value of type V wrapped in an Option depending whether there was a previous
    ///      value
    /// Err - Error of type CacheLoadingError
    pub async fn set(&self, key: K, value: V) -> Result<Option<V>, CacheLoadingError> {
        self.send_cache_action(CacheAction::Set(key, value)).await
    }

    pub async fn get_if_present(&self, key: K) -> Result<Option<V>, CacheLoadingError> {
        self.send_cache_action(CacheAction::GetIfPresent(key)).await
    }

    pub async fn exists(&self, key: K) -> Result<bool, CacheLoadingError> {
        self.get_if_present(key).await
            .map(|result| result.is_some())
    }

    /// Unstable, Undocumented & Inconsistent. Don't use that just yet
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