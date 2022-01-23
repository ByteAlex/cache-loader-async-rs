use std::hash::Hash;
use futures::Future;
use tokio::task::JoinHandle;
use crate::cache_api::{CacheResult, CacheLoadingError, CacheEntry, CacheCommunicationError};
use crate::backing::CacheBacking;
use std::fmt::Debug;

macro_rules! unwrap_backing {
    ($expr:expr) => {
        match $expr {
            Ok(data) => data,
            Err(err) => return CacheResult::Error(err),
        }
    }
}

pub(crate) enum CacheAction<
    K: Clone + Eq + Hash + Send,
    V: Clone + Sized + Send,
    E: Debug + Clone + Send,
    B: CacheBacking<K, CacheEntry<V, E>>
> {
    GetIfPresent(K),
    Get(K),
    Set(K, V, Option<B::Meta>),
    Update(K, Option<B::Meta>, Box<dyn FnOnce(V) -> V + Send + 'static>, bool),
    UpdateMut(K, Box<dyn FnMut(&mut V) -> () + Send + 'static>, bool),
    Remove(K),
    RemoveIf(Box<dyn Fn((&K, Option<&V>)) -> bool + Send + Sync + 'static>),
    Clear(),
    // Internal use
    SetAndUnblock(K, V, Option<B::Meta>),
    Unblock(K),
}

pub(crate) struct CacheMessage<
    K: Eq + Hash + Clone + Send,
    V: Clone + Sized + Send,
    E: Debug + Clone + Send,
    B: CacheBacking<K, CacheEntry<V, E>>
> {
    pub(crate) action: CacheAction<K, V, E, B>,
    pub(crate) response: tokio::sync::oneshot::Sender<CacheResult<V, E>>,
}

pub(crate) struct InternalCacheStore<
    K: Clone + Eq + Hash + Send,
    V: Clone + Sized + Send,
    T,
    E: Debug + Clone + Send,
    B: CacheBacking<K, CacheEntry<V, E>>
> {
    tx: tokio::sync::mpsc::Sender<CacheMessage<K, V, E, B>>,
    data: B,
    loader: T,
}

impl<
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Sized + Send + 'static,
    E: Clone + Sized + Send + Debug + 'static,
    F: Future<Output=Result<V, E>> + Sized + Send + 'static,
    T: Fn(K) -> F + Send + 'static,
    B: CacheBacking<K, CacheEntry<V, E>> + Send + 'static
> InternalCacheStore<K, V, T, E, B>
{
    pub fn new(
        backing: B,
        tx: tokio::sync::mpsc::Sender<CacheMessage<K, V, E, B>>,
        loader: T,
    ) -> Self {
        Self {
            tx,
            data: backing,
            loader,
        }
    }

    pub(crate) fn run(mut self, mut rx: tokio::sync::mpsc::Receiver<CacheMessage<K, V, E, B>>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                if let Some(message) = rx.recv().await {
                    let result = match message.action {
                        CacheAction::GetIfPresent(key) => self.get_if_present(key),
                        CacheAction::Get(key) => self.get(key),
                        CacheAction::Set(key, value, meta) => self.set(key, value, false, meta),
                        CacheAction::Update(key, meta, update_fn, load) => self.update(key, update_fn, load, meta),
                        CacheAction::UpdateMut(key, update_mut_fn, load) => self.update_mut(key, update_mut_fn, load),
                        CacheAction::Remove(key) => self.remove(key),
                        CacheAction::RemoveIf(predicate) => self.remove_if(predicate),
                        CacheAction::Clear() => self.clear(),
                        CacheAction::SetAndUnblock(key, value, meta) => self.set(key, value, true, meta),
                        CacheAction::Unblock(key) => self.unblock(key),
                    };
                    message.response.send(result).ok();
                }
            }
        })
    }

    fn unblock(&mut self, key: K) -> CacheResult<V, E>{
        if let Some(entry) = unwrap_backing!(self.data.get(&key)) {
            if let CacheEntry::Loading(_) = entry {
                if let Some(entry) = unwrap_backing!(self.data.remove(&key)) {
                    if let CacheEntry::Loading(waiter) = entry {
                        std::mem::drop(waiter) // dropping the sender closes the channel
                    }
                }
            }
        }
        CacheResult::None
    }

    fn remove(&mut self, key: K) -> CacheResult<V, E> {
        if let Some(entry) = unwrap_backing!(self.data.remove(&key)) {
            match entry {
                CacheEntry::Loaded(data) => CacheResult::Found(data),
                CacheEntry::Loading(_) => CacheResult::None
            }
        } else {
            CacheResult::None
        }
    }

    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, Option<&V>)) -> bool + Send + Sync + 'static>) -> CacheResult<V, E> {
        unwrap_backing!(self.data.remove_if(self.to_predicate(predicate)));
        CacheResult::None
    }

    fn to_predicate(&self, predicate: Box<dyn Fn((&K, Option<&V>)) -> bool + Send + Sync + 'static>)
                    -> Box<dyn Fn((&K, &CacheEntry<V, E>)) -> bool + Send + Sync + 'static> {
        Box::new(move |(key, value)| {
            match value {
                CacheEntry::Loaded(value) => {
                    predicate((key, Some(value)))
                }
                CacheEntry::Loading(_) => {
                    predicate((key, None))
                }
            }
        })
    }

    fn clear(&mut self) -> CacheResult<V, E> {
        unwrap_backing!(self.data.clear());
        CacheResult::None
    }

    fn update_mut(&mut self, key: K, mut update_mut_fn: Box<dyn FnMut(&mut V) -> () + Send + 'static>, load: bool) -> CacheResult<V, E> {
        match unwrap_backing!(self.data.get_mut(&key)) {
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
                                action: CacheAction::UpdateMut(key, update_mut_fn, load),
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
                if load {
                    let result = self.get(key.clone());
                    match result {
                        CacheResult::Loading(waiter) => {
                            let cache_tx = self.tx.clone();
                            CacheResult::Loading(tokio::spawn(async move {
                                waiter.await.ok(); // result confirmed
                                let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                                cache_tx.send(CacheMessage {
                                    action: CacheAction::UpdateMut(key, update_mut_fn, load),
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
                } else {
                    CacheResult::None
                }
            }
        }
    }

    fn update(&mut self, key: K, update_fn: Box<dyn FnOnce(V) -> V + Send + 'static>, load: bool, meta: Option<B::Meta>) -> CacheResult<V, E> {
        let data = if load {
            self.get(key.clone())
        } else {
            self.get_if_present(key.clone())
        };

        match data {
            CacheResult::Found(data) => {
                let updated_data = update_fn(data);
                unwrap_backing!(self.data.set(key, CacheEntry::Loaded(updated_data.clone()), meta));
                CacheResult::Found(updated_data)
            }
            CacheResult::Loading(handle) => {
                let tx = self.tx.clone();
                CacheResult::Loading(tokio::spawn(async move {
                    handle.await.ok(); // set stupidly await the load to be done
                    // we let the set logic take place which is called from within the future
                    // and we're invoking a second update on the (now cached) data
                    let (response_tx, rx) = tokio::sync::oneshot::channel();
                    tx.send(CacheMessage {
                        action: CacheAction::Update(key, meta, update_fn, load),
                        response: response_tx,
                    }).await.ok();
                    match rx.await {
                        Ok(result) => {
                            match result {
                                CacheResult::Found(data) => Ok(data),
                                CacheResult::Loading(_) => Err(CacheLoadingError::CommunicationError(CacheCommunicationError::LookupLoop())),
                                CacheResult::None => Err(CacheLoadingError::CommunicationError(CacheCommunicationError::LookupLoop())),
                                CacheResult::Error(err) => Err(CacheLoadingError::BackingError(err)),
                            }
                        }
                        Err(err) => Err(CacheLoadingError::CommunicationError(CacheCommunicationError::TokioOneshotRecvError(err))),
                    }
                }))
            }
            res => res
        }
    }

    fn set(&mut self, key: K, value: V, loading_result: bool, meta: Option<B::Meta>) -> CacheResult<V, E> {
        let opt_entry = unwrap_backing!(self.data.get(&key));
        if loading_result {
            if opt_entry.is_none() {
                return CacheResult::None; // abort mission, key was deleted via remove
            }
            let entry = opt_entry.unwrap(); // it's some, because we return if its none
            if matches!(entry, CacheEntry::Loaded(_)) {
                return CacheResult::None; // abort mission, we already have an updated entry!
            }
        }
        unwrap_backing!(self.data.set(key, CacheEntry::Loaded(value), meta))
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
        if let Some(entry) = unwrap_backing!(self.data.get(&key)) {
            match entry {
                CacheEntry::Loaded(data) => CacheResult::Found(data.clone()),
                CacheEntry::Loading(_) => CacheResult::None,
            }
        } else {
            CacheResult::None
        }
    }

    fn get(&mut self, key: K) -> CacheResult<V, E> {
        if let Some(entry) = unwrap_backing!(self.data.get(&key)) {
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
                            Err(err) => Err(CacheLoadingError::CommunicationError(CacheCommunicationError::TokioBroadcastRecvError(err)))
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
                            action: CacheAction::SetAndUnblock(inner_key, send_value, None),
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
            // Loading state is set without any meta
            unwrap_backing!(self.data.set(key, CacheEntry::Loading(tx), None));
            CacheResult::Loading(join_handle)
        }
    }
}