use std::collections::HashMap;
use std::hash::Hash;
#[cfg(feature = "lru-cache")]
use lru::LruCache;
#[cfg(feature = "ttl-cache")]
use std::collections::VecDeque;
#[cfg(feature = "ttl-cache")]
use std::time::{SystemTime, Duration};
#[cfg(feature = "ttl-cache")]
use std::ops::Add;

pub trait CacheBacking<K, V>
    where K: Eq + Hash + Sized + Clone + Send,
          V: Sized + Clone + Send {
    fn get_mut(&mut self, key: &K) -> Option<&mut V>;
    fn get(&mut self, key: &K) -> Option<&V>;
    fn set(&mut self, key: K, value: V) -> Option<V>;
    fn remove(&mut self, key: &K) -> Option<V>;
    fn contains_key(&self, key: &K) -> bool;
    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, &V)) -> bool + Send + 'static>);
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

    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, &V)) -> bool + Send>) {
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
        for key in keys.into_iter(){
            self.lru.pop(&key);
        }
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
    expiry_queue: VecDeque<(K, SystemTime)>,
    map: HashMap<K, (V, SystemTime)>,
}

#[cfg(feature = "ttl-cache")]
impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> CacheBacking<K, V> for TtlCacheBacking<K, V> {
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.remove_old();
        self.map.get_mut(key)
            .map(|(value, _)| value)
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        self.remove_old();
        self.map.get(key)
            .map(|(value, _)| value)
    }

    fn set(&mut self, key: K, value: V) -> Option<V> {
        self.remove_old();
        let expiry = SystemTime::now().add(self.ttl);
        let option = self.map.insert(key.clone(), (value, expiry));
        if option.is_some() {
            self.expiry_queue.retain(|(vec_key, _)| vec_key.ne(&key));
        }
        self.expiry_queue.push_back((key, expiry));
        option.map(|(value, _)| value)
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        self.remove_old();
        let option = self.map.remove(key);
        if option.is_some() {
            self.expiry_queue.retain(|(vec_key, _)| vec_key.ne(&key));
        }
        option.map(|(value, _)| value)
    }

    fn contains_key(&self, key: &K) -> bool {
        // we cant clean old keys on this, since the self ref is not mutable :(
        self.map.get(key)
            .filter(|(_, expiry)| SystemTime::now().lt(expiry))
            .is_some()
    }

    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, &V)) -> bool + Send>) {
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
            self.expiry_queue.retain(|(expiry_key, _)| expiry_key.ne(&key))
        }
    }
}

#[cfg(feature = "ttl-cache")]
impl<K: Hash + Sized + PartialEq + Eq, V> TtlCacheBacking<K, V> {
    pub fn new(ttl: Duration) -> TtlCacheBacking<K, V> {
        TtlCacheBacking {
            ttl,
            map: HashMap::new(),
            expiry_queue: VecDeque::new(),
        }
    }

    fn remove_old(&mut self) {
        let now = SystemTime::now();
        while let Some((key, expiry)) = self.expiry_queue.pop_front() {
            if now.lt(&expiry) {
                self.expiry_queue.push_front((key, expiry));
                break;
            }
            self.map.remove(&key);
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

    fn remove_if(&mut self, predicate: Box<dyn Fn((&K, &V)) -> bool + Send>) {
        self.map.retain(|k, v| !predicate((k, v)));
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