use std::collections::HashMap;
use std::hash::Hash;
use futures::Future;
use tokio::task::JoinHandle;
use crate::cache_api::{CacheResult, CacheLoadingError};

pub(crate) enum CacheAction<K, V> {
    GetIfPresent(K),
    Get(K),
    Set(K, V),
    Update(K, Box<dyn FnOnce(V) -> V + Send + 'static>),
    // todo type U for update function
    // Internal use
    SetAndUnblock(K, V),
    Unblock(K),
}

pub(crate) struct CacheMessage<K, V> {
    pub(crate) action: CacheAction<K, V>,
    pub(crate) response: tokio::sync::oneshot::Sender<CacheResult<V>>,
}

pub(crate) enum CacheEntry<V> {
    Loaded(V),
    Loading(tokio::sync::broadcast::Sender<Option<V>>),
}

pub(crate) struct InternalCacheStore<K, V, T> {
    tx: tokio::sync::mpsc::Sender<CacheMessage<K, V>>,
    data: HashMap<K, CacheEntry<V>>,
    loader: T,
}

impl<
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Sized + Send + 'static,
    F: Future<Output=Option<V>> + Sized + Send + 'static,
    T: Fn(K) -> F + Send + 'static,
> InternalCacheStore<K, V, T>
{
    pub fn new(
        tx: tokio::sync::mpsc::Sender<CacheMessage<K, V>>,
        loader: T,
    ) -> Self {
        Self {
            tx,
            data: Default::default(),
            loader,
        }
    }

    pub(crate) fn run(mut self, mut rx: tokio::sync::mpsc::Receiver<CacheMessage<K, V>>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                if let Some(message) = rx.recv().await {
                    let result = match message.action {
                        CacheAction::GetIfPresent(key) => self.get_if_present(&key),
                        CacheAction::Get(key) => self.get(key),
                        CacheAction::Set(key, value) => self.set(key, value, false),
                        CacheAction::Update(key, update_fn) => self.update(key, update_fn),
                        CacheAction::SetAndUnblock(key, value) => self.set(key, value, true),
                        CacheAction::Unblock(key) => {
                            self.unblock(key);
                            CacheResult::None
                        }
                    };
                    message.response.send(result).ok();
                }
            }
        })
    }

    fn unblock(&mut self, key: K) {
        if let Some(entry) = self.data.get(&key) {
            if let CacheEntry::Loading(waiter) = entry {
                waiter.send(None).ok();
                self.data.remove(&key);
            }
        }
    }

    fn update(&mut self, key: K, update_fn: Box<dyn FnOnce(V) -> V + Send + 'static>) -> CacheResult<V> {
        match self.get(key.clone()) {
            CacheResult::Found(data) => {
                let updated_data = update_fn(data);
                self.data.insert(key, CacheEntry::Loaded(updated_data.clone()));
                CacheResult::Found(updated_data)
            }
            CacheResult::Loading(handle) => {
                CacheResult::Loading(tokio::spawn(async move {
                    handle.await.unwrap() // todo: what now?
                }))
            }
            CacheResult::None => CacheResult::None
        }
    }

    fn set(&mut self, key: K, value: V, loading_result: bool) -> CacheResult<V> {
        if loading_result && self.data.contains_key(&key) {
            return CacheResult::None; // abort mission, we already have an updated entry!
        }
        self.data.insert(key, CacheEntry::Loaded(value))
            .and_then(|entry| {
                match entry {
                    CacheEntry::Loaded(data) => Some(data),
                    CacheEntry::Loading(_) => None
                }
            })
            .map(|value| CacheResult::Found(value))
            .unwrap_or(CacheResult::None)
    }

    fn get_if_present(&mut self, key: &K) -> CacheResult<V> {
        if let Some(entry) = self.data.get(key) {
            match entry {
                CacheEntry::Loaded(data) => CacheResult::Found(data.clone()),
                CacheEntry::Loading(_) => CacheResult::None, // todo: Are we treating Loading as present or not?
            }
        } else {
            CacheResult::None
        }
    }

    fn get(&mut self, key: K) -> CacheResult<V> {
        match self.data.entry(key.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                match entry.get() {
                    CacheEntry::Loaded(value) => {
                        CacheResult::Found(value.clone())
                    }
                    CacheEntry::Loading(waiter) => {
                        let waiter = waiter.clone();
                        CacheResult::Loading(tokio::spawn(async move {
                            if let Ok(result) = waiter.subscribe().recv().await {
                                if let Some(data) = result {
                                    Ok(data)
                                } else {
                                    Err(CacheLoadingError { reason_phrase: "Loader function returned None".to_owned() })
                                }
                            } else {
                                Err(CacheLoadingError { reason_phrase: "Waiter broadcast channel error".to_owned() })
                            }
                        }))
                    }
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let (tx, _) = tokio::sync::broadcast::channel(1);
                let inner_tx = tx.clone();
                let cache_tx = self.tx.clone();
                let loader = (self.loader)(key.clone());
                let key = key.clone();
                let join_handle = tokio::spawn(async move {
                    if let Some(value) = loader.await {
                        inner_tx.send(Some(value.clone())).ok();
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let send_value = value.clone();
                        cache_tx.send(CacheMessage {
                            action: CacheAction::SetAndUnblock(key, send_value),
                            response: tx,
                        }).await.ok();
                        rx.await.ok(); // await cache confirmation
                        Ok(value)
                    } else {
                        inner_tx.send(None).ok();
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        cache_tx.send(CacheMessage {
                            action: CacheAction::Unblock(key),
                            response: tx,
                        }).await.ok();
                        rx.await.ok(); // await cache confirmation
                        Err(CacheLoadingError { reason_phrase: "Loader function returned None".to_owned() })
                    }
                });
                entry.insert(CacheEntry::Loading(tx));
                CacheResult::Loading(join_handle)
            }
        }
    }
}