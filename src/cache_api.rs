use tokio::task::JoinHandle;
use std::hash::Hash;
use futures::Future;
use crate::internal_cache::{CacheAction, InternalCacheStore, CacheMessage};
use crate::backing::{CacheBacking, HashMapBacking};

#[derive(Debug, Clone)]
pub struct CacheLoadingError {
    pub reason_phrase: String,
    pub loading_error: Option<LoadingError>
    // todo: nested errors
}

#[derive(Debug, Clone)]
pub struct LoadingError {
    error_code: i16,
    reason_phrase: Option<String>,
}

impl LoadingError {

    pub const LOADER_INTERNAL_ERROR: i16 = -1;

    pub fn new(error_code: i16) -> LoadingError {
        LoadingError {
            error_code,
            reason_phrase: None
        }
    }

    pub fn with_reason_phrase(error_code: i16, reason_phrase: String) -> LoadingError {
        LoadingError {
            error_code,
            reason_phrase: Some(reason_phrase)
        }
    }

    pub fn get_error_code(&self) -> i16 {
        self.error_code
    }

    pub fn get_reason(&self) -> Option<String> {
        self.reason_phrase.clone()
    }
}

#[derive(Debug, Clone)]
pub enum CacheEntry<V> {
    Loaded(V),
    Loading(tokio::sync::broadcast::Sender<Result<V, LoadingError>>),
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
    /// Creates a new instance of a LoadingCache with the default `HashMapBacking`
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
    /// use cache_loader_async::cache_api::{LoadingCache, LoadingError};
    /// use std::collections::HashMap;
    /// async fn example() {
    ///     let static_db: HashMap<String, u32> =
    ///         vec![("foo".into(), 32), ("bar".into(), 64)]
    ///             .into_iter()
    ///             .collect();
    ///
    ///     let (cache, _) = LoadingCache::new(move |key: String| {
    ///         let db_clone = static_db.clone();
    ///         async move {
    ///             db_clone.get(&key).cloned().ok_or(LoadingError::new(1))
    ///         }
    ///     });
    ///
    ///     let result = cache.get("foo".to_owned()).await.unwrap();
    ///
    ///     assert_eq!(result, 32);
    /// }
    /// ```
    pub fn new<T, F>(loader: T) -> (LoadingCache<K, V>, CacheHandle)
        where F: Future<Output=Result<V, LoadingError>> + Sized + Send + 'static,
              T: Fn(K) -> F + Send + 'static {
        LoadingCache::with_backing(HashMapBacking::new(), loader)
    }

    /// Creates a new instance of a LoadingCache with a custom `CacheBacking`
    ///
    /// # Arguments
    ///
    /// * `backing` - The custom backing which the cache should use
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
    /// use cache_loader_async::cache_api::{LoadingCache, LoadingError};
    /// use std::collections::HashMap;
    /// use cache_loader_async::backing::HashMapBacking;
    /// async fn example() {
    ///     let static_db: HashMap<String, u32> =
    ///         vec![("foo".into(), 32), ("bar".into(), 64)]
    ///             .into_iter()
    ///             .collect();
    ///
    ///     let (cache, _) = LoadingCache::with_backing(
    ///         HashMapBacking::new(), // this is the default implementation of `new`
    ///         move |key: String| {
    ///             let db_clone = static_db.clone();
    ///             async move {
    ///                 db_clone.get(&key).cloned().ok_or(LoadingError::new(1))
    ///             }
    ///         }
    ///     );
    ///
    ///     let result = cache.get("foo".to_owned()).await.unwrap();
    ///
    ///     assert_eq!(result, 32);
    /// }
    /// ```
    pub fn with_backing<T, F, B>(backing: B, loader: T) -> (LoadingCache<K, V>, CacheHandle)
        where F: Future<Output=Result<V, LoadingError>> + Sized + Send + 'static,
              T: Fn(K) -> F + Send + 'static,
              B: CacheBacking<K, CacheEntry<V>> + Send + 'static {
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let store = InternalCacheStore::new(backing, tx.clone(), loader);
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

    /// Loads the value for the specified key from the cache and returns None if not present
    ///
    /// # Arguments
    ///
    /// * `key` - The key which should be loaded
    ///
    /// # Return Value
    ///
    /// Returns a Result with:
    /// Ok - Value of type Option<V>
    /// Err - Error of type CacheLoadingError
    pub async fn get_if_present(&self, key: K) -> Result<Option<V>, CacheLoadingError> {
        self.send_cache_action(CacheAction::GetIfPresent(key)).await
    }

    /// Checks whether a specific value is mapped for the given key
    ///
    /// # Arguments
    ///
    /// * `key` - The key which should be checked
    ///
    /// # Return Value
    ///
    /// Returns a Result with:
    /// Ok - bool
    /// Err - Error of type CacheLoadingError
    pub async fn exists(&self, key: K) -> Result<bool, CacheLoadingError> {
        self.get_if_present(key).await
            .map(|result| result.is_some())
    }

    /// Removes a specific key-value mapping from the cache and returns the previous result
    /// if there was any or None
    ///
    /// # Arguments
    ///
    /// * `key` - The key which should be evicted
    ///
    /// # Return Value
    ///
    /// Returns a Result with:
    /// Ok - Value of type Option<V>
    /// Err - Error of type CacheLoadingError
    pub async fn remove(&self, key: K) -> Result<Option<V>, CacheLoadingError> {
        self.send_cache_action(CacheAction::Remove(key)).await
    }

    /// Updates a key on the cache with the given update function and returns the previous value
    ///
    /// If the key is not present yet, it'll be loaded using the loader function and will be
    /// updated once this loader function completes.
    /// In case the key was manually updated via `set` during the loader function the update will
    /// take place on the manually updated value, so user-controlled input takes precedence over
    /// the loader function
    ///
    /// # Arguments
    ///
    /// * `key` - The key which should be updated
    /// * `update_fn` - A `FnOnce(V) -> V` which has the current value as parameter and should
    ///                 return the updated value
    ///
    /// # Return Value
    ///
    /// Returns a Result with:
    /// Ok - Value of type V which is the previously mapped value
    /// Err - Error of type CacheLoadingError
    pub async fn update<U>(&self, key: K, update_fn: U) -> Result<V, CacheLoadingError>
        where U: FnOnce(V) -> V + Send + 'static {
        self.send_cache_action(CacheAction::Update(key, Box::new(update_fn))).await
            .map(|opt_result| opt_result.expect("Get should always return either V or CacheLoadingError"))
    }

    pub async fn update_mut<U>(&self, key: K, update_fn: U) -> Result<V, CacheLoadingError>
        where U: FnMut(&mut V) -> () + Send + 'static {
        self.send_cache_action(CacheAction::UpdateMut(key, Box::new(update_fn))).await
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
                                        Err(CacheLoadingError {
                                            reason_phrase: "Error when trying to join loader future".to_owned(),
                                            loading_error: None
                                        })
                                    }
                                }
                            }
                            CacheResult::None => { Ok(None) }
                        }
                    }
                    Err(_) => {
                        Err(CacheLoadingError {
                            reason_phrase: "Error when receiving cache response".to_owned(),
                            loading_error: None
                        })
                    }
                }
            }
            Err(_) => {
                Err(CacheLoadingError {
                    reason_phrase: "Error when trying to submit cache request".to_owned(),
                    loading_error: None
                })
            }
        }
    }
}