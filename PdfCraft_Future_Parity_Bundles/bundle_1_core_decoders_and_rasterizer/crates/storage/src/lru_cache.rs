//! Bounded Memory LRU Cache for PDF Object Graph.
//! Tracks memory footprint in bytes and evicts cold objects when budget is exceeded.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub trait SizedObject {
    fn estimated_size_bytes(&self) -> usize;
}

impl SizedObject for Vec<u8> {
    fn estimated_size_bytes(&self) -> usize {
        self.capacity() + std::mem::size_of::<Vec<u8>>()
    }
}

pub struct BoundedLruCache<K, V> {
    map: HashMap<K, V>,
    order: VecDeque<K>,
    current_memory_bytes: usize,
    max_memory_bytes: usize,
}

impl<K: Clone + Eq + Hash, V: SizedObject> BoundedLruCache<K, V> {
    pub fn new(max_memory_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            current_memory_bytes: 0,
            max_memory_bytes,
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.map.contains_key(key) {
            // Move to back (most recently used)
            if let Some(pos) = self.order.iter().position(|k| k == key) {
                self.order.remove(pos);
                self.order.push_back(key.clone());
            }
            self.map.get(key)
        } else {
            None
        }
    }

    pub fn insert(&mut self, key: K, value: V) {
        let size = value.estimated_size_bytes();

        // Evict until new item fits within budget
        while self.current_memory_bytes + size > self.max_memory_bytes && !self.order.is_empty() {
            if let Some(oldest_key) = self.order.pop_front() {
                if let Some(old_val) = self.map.remove(&oldest_key) {
                    self.current_memory_bytes = self.current_memory_bytes.saturating_sub(old_val.estimated_size_bytes());
                }
            }
        }

        // If replacing existing key
        if let Some(existing) = self.map.insert(key.clone(), value) {
            self.current_memory_bytes = self.current_memory_bytes.saturating_sub(existing.estimated_size_bytes());
            if let Some(pos) = self.order.iter().position(|k| *k == key) {
                self.order.remove(pos);
            }
        }

        self.current_memory_bytes += size;
        self.order.push_back(key);
    }

    pub fn current_memory_bytes(&self) -> usize {
        self.current_memory_bytes
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounded_lru_eviction() {
        // 150 bytes budget (each 40-byte Vec has capacity 40 + 24 bytes overhead = 64 bytes)
        let mut cache = BoundedLruCache::new(150);
        let obj1 = vec![0u8; 40];
        let obj2 = vec![1u8; 40];
        let obj3 = vec![2u8; 40];

        cache.insert(1, obj1);
        cache.insert(2, obj2);
        assert_eq!(cache.len(), 2);

        // Inserting obj3 should evict obj1
        cache.insert(3, obj3);
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&2).is_some());
        assert!(cache.get(&3).is_some());
    }
}
