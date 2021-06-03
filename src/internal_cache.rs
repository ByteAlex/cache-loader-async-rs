use std::hash::Hash;
use futures::Future;
use tokio::task::JoinHandle;
use crate::cache_api::{CacheResult, CacheLoadingError, CacheEntry, LoadingError};
use crate::backing::CacheBacking;
use std::fmt::Debug;

pub(crate) enum CacheAction<K, V> {
    GetIfPresent(K),
    Get(K),
    Set(K, V),
    Update(K, Box<dyn FnOnce(V) -> V + Send + 'static>),
    UpdateMut(K, Box<dyn FnMut(&mut V) -> () + Send + 'static>),
    Remove(K),
    // Internal use
    SetAndUnblock(K, V),
    Unblock(K),
}

pub(crate) struct CacheMessage<K, V, E: Debug> {
    pub(crate) action: CacheAction<K, V>,
    pub(crate) response: tokio::sync::oneshot::Sender<CacheResult<V, E>>,
}

pub(crate) struct InternalCacheStore<K, V, T, B, E: Debug> {
    tx: tokio::sync::mpsc::Sender<CacheMessage<K, V, E>>,
    data: B,
    loader: T,
}

impl<
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Sized + Send + 'static,
    E: Clone + Sized + Send + Debug + 'static,
    F: Future<Output=Result<V, LoadingError<E>>> + Sized + Send + 'static,
    T: Fn(K) -> F + Send + 'static,
    B: CacheBacking<K, CacheEntry<V, E>> + Send + 'static
> InternalCacheStore<K, V, T, B, E>
{
    pub fn new(
        backing: B,
        tx: tokio::sync::mpsc::Sender<CacheMessage<K, V, E>>,
        loader: T,
    ) -> Self {
        Self {
            tx,
            data: backing,
            loader,
        }
    }

    pub(crate) fn run(mut self, mut rx: tokio::sync::mpsc::Receiver<CacheMessage<K, V, E>>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                if let Some(message) = rx.recv().await {
                    let result = match message.action {
                        CacheAction::GetIfPresent(key) => self.get_if_present(key),
                        CacheAction::Get(key) => self.get(key),
                        CacheAction::Set(key, value) => self.set(key, value, false),
                        CacheAction::Update(key, update_fn) => self.update(key, update_fn),
                        CacheAction::UpdateMut(key, update_mut_fn) => self.update_mut(key, update_mut_fn),
                        CacheAction::Remove(key) => self.remove(key),
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
                waiter.send(Err(LoadingError::LoadCancelled())).ok();
                self.data.remove(&key);
            }
        }
    }

    fn remove(&mut self, key: K) -> CacheResult<V, E> {
        if let Some(entry) = self.data.remove(&key) {
            match entry {
                CacheEntry::Loaded(data) => CacheResult::Found(data),
                CacheEntry::Loading(_) => CacheResult::None
            }
        } else {
            CacheResult::None
        }
    }

    fn update_mut(&mut self, key: K, mut update_mut_fn: Box<dyn FnMut(&mut V) -> () + Send + 'static>) -> CacheResult<V, E> {
        match self.data.get_mut(&key) {
            Some(entry) => {
                match entry {
                    CacheEntry::Loaded(data) => {
                        update_mut_fn(data);
                        CacheResult::Found(data.clone())
                    }
                    CacheEntry::Loading(waiter) => {
                        let mut rx = waiter.subscribe();
                        let cache_tx = self.tx.clone();
                        CacheResult::Loading(tokio::spawn(async move {
                            rx.recv().await.ok(); // result confirmed
                            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                            cache_tx.send(CacheMessage {
                                action: CacheAction::UpdateMut(key, update_mut_fn),
                                response: response_tx,
                            }).await.ok();
                            match response_rx.await.unwrap() {
                                CacheResult::Found(data) => Ok(data),
                                _ => Err(CacheLoadingError::NoData())
                            }
                        }))
                    }
                }
            }
            None => {
                let result = self.get(key.clone());
                match result {
                    CacheResult::Loading(waiter) => {
                        let cache_tx = self.tx.clone();
                        CacheResult::Loading(tokio::spawn(async move {
                            waiter.await.ok(); // result confirmed
                            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                            cache_tx.send(CacheMessage {
                                action: CacheAction::UpdateMut(key, update_mut_fn),
                                response: response_tx,
                            }).await.ok();
                            match response_rx.await.unwrap() {
                                CacheResult::Found(data) => Ok(data),
                                _ => Err(CacheLoadingError::NoData())
                            }
                        }))
                    }
                    _ => CacheResult::None,
                }
            }
        }
    }

    fn update(&mut self, key: K, update_fn: Box<dyn FnOnce(V) -> V + Send + 'static>) -> CacheResult<V, E> {
        match self.get(key.clone()) {
            CacheResult::Found(data) => {
                let updated_data = update_fn(data);
                self.data.set(key, CacheEntry::Loaded(updated_data.clone()));
                CacheResult::Found(updated_data)
            }
            CacheResult::Loading(handle) => {
                let tx = self.tx.clone();
                CacheResult::Loading(tokio::spawn(async move {
                    handle.await.ok(); // set stupidly await the load to be done
                    // we let the set logic take place which is called from within the future
                    // and we're invoking a second update on the (now cached) data
                    // todo: is there a possibility that this loops forever?
                    let (response_tx, rx) = tokio::sync::oneshot::channel();
                    tx.send(CacheMessage {
                        action: CacheAction::Update(key, update_fn),
                        response: response_tx,
                    }).await.ok();
                    match rx.await {
                        Ok(result) => {
                            match result {
                                CacheResult::Found(data) => Ok(data),
                                CacheResult::Loading(_) => Err(CacheLoadingError::LookupLoop()),
                                CacheResult::None => Err(CacheLoadingError::LookupLoop())
                            }
                        }
                        Err(err) => Err(CacheLoadingError::TokioOneshotRecvError(err)),
                    }
                }))
            }
            CacheResult::None => CacheResult::None
        }
    }

    fn set(&mut self, key: K, value: V, loading_result: bool) -> CacheResult<V, E> {
        let opt_entry = self.data.get(&key);
        if loading_result {
            if opt_entry.is_none() {
                return CacheResult::None; // abort mission, key was deleted via remove
            }
            let entry = opt_entry.unwrap(); // it's some, because we return if its none
            if matches!(entry, CacheEntry::Loaded(_)) {
                return CacheResult::None; // abort mission, we already have an updated entry!
            }
        }
        self.data.set(key, CacheEntry::Loaded(value))
            .and_then(|entry| {
                match entry {
                    CacheEntry::Loaded(data) => Some(data),
                    CacheEntry::Loading(_) => None
                }
            })
            .map(|value| CacheResult::Found(value))
            .unwrap_or(CacheResult::None)
    }

    fn get_if_present(&mut self, key: K) -> CacheResult<V, E> {
        if let Some(entry) = self.data.get(&key) {
            match entry {
                CacheEntry::Loaded(data) => CacheResult::Found(data.clone()),
                CacheEntry::Loading(_) => CacheResult::None, // todo: Are we treating Loading as present or not?
            }
        } else {
            CacheResult::None
        }
    }

    fn get(&mut self, key: K) -> CacheResult<V, E> {
        if let Some(entry) = self.data.get(&key) {
            match entry {
                CacheEntry::Loaded(value) => {
                    CacheResult::Found(value.clone())
                }
                CacheEntry::Loading(waiter) => {
                    let waiter = waiter.clone();
                    CacheResult::Loading(tokio::spawn(async move {
                        match waiter.subscribe().recv().await {
                            Ok(result) => {
                                match result {
                                    Ok(data) => {
                                        Ok(data)
                                    }
                                    Err(loading_error) => {
                                        Err(CacheLoadingError::LoadingError(loading_error))
                                    }
                                }
                            }
                            Err(err) => Err(CacheLoadingError::TokioBroadcastRecvError(err))
                        }
                    }))
                }
            }
        } else {
            let (tx, _) = tokio::sync::broadcast::channel(1);
            let inner_tx = tx.clone();
            let cache_tx = self.tx.clone();
            let loader = (self.loader)(key.clone());
            let inner_key = key.clone();
            let join_handle = tokio::spawn(async move {
                match loader.await {
                    Ok(value) => {
                        inner_tx.send(Ok(value.clone())).ok();
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let send_value = value.clone();
                        cache_tx.send(CacheMessage {
                            action: CacheAction::SetAndUnblock(inner_key, send_value),
                            response: tx,
                        }).await.ok();
                        rx.await.ok(); // await cache confirmation
                        Ok(value)
                    }
                    Err(loading_error) => {
                        inner_tx.send(Err(loading_error.clone())).ok();
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        cache_tx.send(CacheMessage {
                            action: CacheAction::Unblock(inner_key),
                            response: tx,
                        }).await.ok();
                        rx.await.ok(); // await cache confirmation
                        Err(CacheLoadingError::LoadingError(loading_error))
                    }
                }
            });
            self.data.set(key, CacheEntry::Loading(tx));
            CacheResult::Loading(join_handle)
        }
    }
}