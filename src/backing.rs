use std::collections::HashMap;
use std::hash::Hash;
#[cfg(feature = "lru-cache")]
use lru::LruCache;
#[cfg(feature = "ttl-cache")]
use moka::sync::Cache;

pub trait CacheBacking<K, V>
    where K: Eq + Hash + Sized + Clone + Send,
          V: Sized + Clone + Send {
    fn get_mut(&mut self, key: &K) -> Option<&mut V>;
    fn get(&mut self, key: &K) -> Option<&V>;
    fn set(&mut self, key: K, value: V) -> Option<V>;
    fn remove(&mut self, key: &K) -> Option<V>;
    fn contains_key(&self, key: &K) -> bool;
}

#[cfg(feature = "lru-cache")]
pub struct LruCacheBacking<K, V> {
    lru: LruCache<K, V>
}

#[cfg(feature = "lru-cache")]
impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> CacheBacking<K, V> for LruCacheBacking<K, V> {
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.lru.get_mut(key)
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        self.lru.get(key)
    }

    fn set(&mut self, key: K, value: V) -> Option<V> {
        self.lru.put(key, value)
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        self.lru.pop(key)
    }

    fn contains_key(&self, key: &K) -> bool {
        self.lru.contains(&key.clone())
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
pub struct TtlBacking<K, V> {
    cache: Cache<K, V>
}

#[cfg(feature = "ttl-cache")]
impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> CacheBacking<K, V> for TtlBacking<K, V> {
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.cache.get(&key).as_mut()
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        self.cache.get(&key)
    }

    fn set(&mut self, key: K, value: V) -> Option<V> {
        let prev = self.cache.get(&key);
        self.cache.insert(key, value);
        prev
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        let prev = self.cache.get(&key);
        self.cache.invalidate(&key);
        prev
    }

    fn contains_key(&self, key: &K) -> bool {
        self.cache.get(&key).is_some()
    }
}

#[cfg(feature = "ttl-cache")]
impl<K, V> TtlBacking<K, V> {

    pub fn new(moka_cache: Cache<K, V>) -> TtlBacking<K, V> {
        TtlBacking {
            cache: moka_cache
        }
    }
}

pub struct HashMapBacking<K, V> {
    map: HashMap<K, V>
}

impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> CacheBacking<K, V> for HashMapBacking<K, V> {
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.map.get_mut(key)
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    fn set(&mut self, key: K, value: V) -> Option<V> {
        self.map.insert(key, value)
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        self.map.remove(key)
    }

    fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
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