use std::collections::HashMap;
use std::hash::Hash;
#[cfg(feature = "lru-cache")]
use lru::LruCache;
#[cfg(feature = "ttl-cache")]
use std::collections::VecDeque;
use std::fmt::Debug;
use thiserror::Error;
#[cfg(feature = "ttl-cache")]
use std::ops::Add;
#[cfg(feature = "ttl-cache")]
use tokio::time::{Instant, Duration};

pub trait CacheBacking<K, V>
    where K: Eq + Hash + Sized + Clone + Send,
          V: Sized + Clone + Send {
    type Meta: Clone + Send;

    fn get_mut(&mut self, key: &K) -> Result<Option<&mut V>, BackingError>;
    fn get(&mut self, key: &K) -> Result<Option<&V>, BackingError>;
    fn set(&mut self, key: K, value: V, meta: Option<Self::Meta>) -> Result<Option<V>, BackingError>;
    fn remove(&mut self, key: &K) -> Result<Option<V>, BackingError>;
    fn contains_key(&self, key: &K) -> Result<bool, BackingError>;
    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, &V)) -> bool + Send + 'static>) -> Result<(), BackingError>;
    fn clear(&mut self) -> Result<(), BackingError>;
}

#[derive(Debug, Clone, Error)]
pub enum BackingError {
    #[error(transparent)]
    TtlError(#[from] TtlError),
}

#[derive(Copy, Clone, Debug, Default)]
pub struct NoMeta {}

#[cfg(feature = "lru-cache")]
pub struct LruCacheBacking<K, V> {
    lru: LruCache<K, V>,
}

#[cfg(feature = "lru-cache")]
impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> CacheBacking<K, V> for LruCacheBacking<K, V> {
    type Meta = NoMeta;

    fn get_mut(&mut self, key: &K) -> Result<Option<&mut V>, BackingError> {
        Ok(self.lru.get_mut(key))
    }

    fn get(&mut self, key: &K) -> Result<Option<&V>, BackingError> {
        Ok(self.lru.get(key))
    }

    fn set(&mut self, key: K, value: V, _meta: Option<Self::Meta>) -> Result<Option<V>, BackingError> {
        Ok(self.lru.put(key, value))
    }

    fn remove(&mut self, key: &K) -> Result<Option<V>, BackingError> {
        Ok(self.lru.pop(key))
    }

    fn contains_key(&self, key: &K) -> Result<bool, BackingError> {
        Ok(self.lru.contains(&key.clone()))
    }

    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, &V)) -> bool + Send>) -> Result<(), BackingError> {
        let keys = self.lru.iter()
            .filter_map(|(key, value)| {
                if predicate((key, value)) {
                    Some(key)
                } else {
                    None
                }
            })
            .cloned()
            .collect::<Vec<K>>();
        for key in keys.into_iter() {
            self.lru.pop(&key);
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<(), BackingError> {
        self.lru.clear();
        Ok(())
    }
}

#[cfg(feature = "lru-cache")]
impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> LruCacheBacking<K, V> {
    pub fn new(size: usize) -> LruCacheBacking<K, V> {
        LruCacheBacking {
            lru: LruCache::new(size)
        }
    }

    pub fn unbounded() -> LruCacheBacking<K, V> {
        LruCacheBacking {
            lru: LruCache::unbounded()
        }
    }
}

#[cfg(feature = "ttl-cache")]
pub struct TtlCacheBacking<K, V> {
    ttl: Duration,
    expiry_queue: VecDeque<TTlEntry<K>>,
    map: HashMap<K, (V, Instant)>,
}

#[cfg(feature = "ttl-cache")]
struct TTlEntry<K> {
    key: K,
    expiry: Instant,

}

#[cfg(feature = "ttl-cache")]
impl<K> From<(K, Instant)> for TTlEntry<K> {
    fn from(tuple: (K, Instant)) -> Self {
        Self {
            key: tuple.0,
            expiry: tuple.1,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum TtlError {
    #[error("The expiry for key not found")]
    ExpiryNotFound,
    #[error("No key for expiry matched key")]
    ExpiryKeyNotFound,
}

#[cfg(feature = "ttl-cache")]
#[derive(Debug, Copy, Clone)]
pub struct TtlMeta {
    pub ttl: Duration,
}

#[cfg(feature = "ttl-cache")]
impl From<Duration> for TtlMeta {
    fn from(ttl: Duration) -> Self {
        Self { ttl }
    }
}

#[cfg(feature = "ttl-cache")]
impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> CacheBacking<K, V> for TtlCacheBacking<K, V> {
    type Meta = TtlMeta;

    fn get_mut(&mut self, key: &K) -> Result<Option<&mut V>, BackingError> {
        self.remove_old();
        Ok(self.map.get_mut(key)
            .map(|(value, _)| value))
    }

    fn get(&mut self, key: &K) -> Result<Option<&V>, BackingError> {
        self.remove_old();
        Ok(self.map.get(key)
            .map(|(value, _)| value))
    }

    fn set(&mut self, key: K, value: V, meta: Option<Self::Meta>) -> Result<Option<V>, BackingError> {
        self.remove_old();
        let ttl = if let Some(meta) = meta {
            meta.ttl
        } else {
            self.ttl
        };
        let expiry = Instant::now().add(ttl);
        let result = self.replace(key.clone(), value, expiry)?;
        Ok(result)
    }

    fn remove(&mut self, key: &K) -> Result<Option<V>, BackingError> {
        self.remove_old();
        Ok(self.remove_key(key)?)
    }

    fn contains_key(&self, key: &K) -> Result<bool, BackingError> {
        // we cant clean old keys on this, since the self ref is not mutable :(
        Ok(self.map.get(key)
            .filter(|(_, expiry)| Instant::now().lt(expiry))
            .is_some())
    }

    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, &V)) -> bool + Send>) -> Result<(), BackingError> {
        let keys = self.map.iter()
            .filter_map(|(key, (value, _))| {
                if predicate((key, value)) {
                    Some(key)
                } else {
                    None
                }
            })
            .cloned()
            .collect::<Vec<K>>();
        for key in keys.into_iter() {
            self.map.remove(&key);
            // optimize looping through expiry_queue multiple times?
            self.expiry_queue.retain(|entry| entry.key.ne(&key))
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<(), BackingError> {
        self.expiry_queue.clear();
        self.map.clear();
        Ok(())
    }
}

#[cfg(feature = "ttl-cache")]
impl<K: Eq + Hash + Sized + Clone + Send, V: Sized + Clone + Send> TtlCacheBacking<K, V> {
    pub fn new(ttl: Duration) -> TtlCacheBacking<K, V> {
        TtlCacheBacking {
            ttl,
            map: HashMap::new(),
            expiry_queue: VecDeque::new(),
        }
    }

    fn remove_old(&mut self) {
        let now = Instant::now();
        while let Some(entry) = self.expiry_queue.pop_front() {
            if now.lt(&entry.expiry) {
                self.expiry_queue.push_front(entry);
                break;
            }
            self.map.remove(&entry.key);
        }
    }

    fn replace(&mut self, key: K, value: V, expiry: Instant) -> Result<Option<V>, TtlError> {
        let entry = self.map.insert(key.clone(), (value, expiry));
        let res = self.cleanup_expiry(entry, &key);
        match self.expiry_queue.binary_search_by_key(&expiry, |entry| entry.expiry) {
            Ok(found) => {
                self.expiry_queue.insert(found + 1, (key, expiry).into());
            }
            Err(idx) => {
                self.expiry_queue.insert(idx, (key, expiry).into());
            }
        }
        res
    }

    fn remove_key(&mut self, key: &K) -> Result<Option<V>, TtlError> {
        let entry = self.map.remove(key);
        self.cleanup_expiry(entry, key)
    }

    fn cleanup_expiry(&mut self, entry: Option<(V, Instant)>, key: &K) -> Result<Option<V>, TtlError> {
        if let Some((value, old_expiry)) = entry {
            match self.expiry_queue.binary_search_by_key(&old_expiry, |entry| entry.expiry) {
                Ok(found) => {
                    let index = self.expiry_index_on_key_eq(found, &old_expiry, key);
                    if let Some(index) = index {
                        self.expiry_queue.remove(index);
                    } else {
                        return Err(TtlError::ExpiryKeyNotFound);
                    }
                }
                Err(_) => {
                    return Err(TtlError::ExpiryNotFound);
                }
            }
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    fn expiry_index_on_key_eq(&self, idx: usize, expiry: &Instant, key: &K) -> Option<usize> {
        let entry = self.expiry_queue.get(idx).unwrap();
        if entry.key.eq(key) {
            return Some(idx);
        }

        let mut offset = 0;
        while idx - offset > 0 {
            offset += 1;
            let entry = self.expiry_queue.get(idx - offset).unwrap();
            if !entry.expiry.eq(expiry) {
                break;
            }
            if entry.key.eq(key) {
                return Some(idx - offset);
            }
        }
        offset = 0;
        while idx + offset < self.expiry_queue.len() {
            offset += 1;
            let entry = self.expiry_queue.get(idx + offset).unwrap();
            if !entry.expiry.eq(expiry) {
                break;
            }
            if entry.key.eq(key) {
                return Some(idx + offset);
            }
        }
        None
    }
}

pub struct HashMapBacking<K, V> {
    map: HashMap<K, V>,
}

impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> CacheBacking<K, V> for HashMapBacking<K, V> {
    type Meta = NoMeta;

    fn get_mut(&mut self, key: &K) -> Result<Option<&mut V>, BackingError> {
        Ok(self.map.get_mut(key))
    }

    fn get(&mut self, key: &K) -> Result<Option<&V>, BackingError> {
        Ok(self.map.get(key))
    }

    fn set(&mut self, key: K, value: V, _meta: Option<Self::Meta>) -> Result<Option<V>, BackingError> {
        Ok(self.map.insert(key, value))
    }

    fn remove(&mut self, key: &K) -> Result<Option<V>, BackingError> {
        Ok(self.map.remove(key))
    }

    fn contains_key(&self, key: &K) -> Result<bool, BackingError> {
        Ok(self.map.contains_key(key))
    }

    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, &V)) -> bool + Send>) -> Result<(), BackingError> {
        self.map.retain(|k, v| !predicate((k, v)));
        Ok(())
    }

    fn clear(&mut self) -> Result<(), BackingError> {
        self.map.clear();
        Ok(())
    }
}

impl<K, V> HashMapBacking<K, V> {
    pub fn new() -> HashMapBacking<K, V> {
        HashMapBacking {
            map: Default::default()
        }
    }

    pub fn construct(map: HashMap<K, V>) -> HashMapBacking<K, V> {
        HashMapBacking {
            map
        }
    }
}