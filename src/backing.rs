use std::collections::HashMap;
use std::hash::Hash;

pub trait CacheBacking<K, V>
    where K: Eq + Hash + Sized + Clone + Send,
          V: Sized + Clone + Send {
    fn get(&mut self, key: K) -> Option<V>;
    fn set(&mut self, key: K, value: V) -> Option<V>;
    fn remove(&mut self, key: K) -> Option<V>;
    fn contains_key(&self, key: &K) -> bool;
}

pub struct HashMapBacking<K, V> {
    map: HashMap<K, V>
}

impl<
    K: Eq + Hash + Sized + Clone + Send,
    V: Sized + Clone + Send
> CacheBacking<K, V> for HashMapBacking<K, V> {
    fn get(&mut self, key: K) -> Option<V> {
        self.map.get(&key).cloned()
    }

    fn set(&mut self, key: K, value: V) -> Option<V> {
        self.map.insert(key, value)
    }

    fn remove(&mut self, key: K) -> Option<V> {
        self.map.remove(&key)
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