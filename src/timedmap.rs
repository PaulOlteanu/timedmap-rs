use dashmap::DashMap;

use crate::{time::TimeSource, Cleanup, Value};
use std::{
    borrow::Borrow,
    hash::Hash,
    time::{Duration, Instant},
};

/// Provides a hash map with expiring key-value pairs.
///
/// # Basic Example
/// ```
/// use timedmap::TimedMap;
/// use std::time::Duration;
///
/// let tm = TimedMap::new();
/// tm.insert("foo", "bar", Duration::from_secs(10));
/// assert_eq!(tm.get(&"foo"), Some("bar"));
/// ```
pub struct TimedMap<K, V, TS = Instant> {
    inner: DashMap<K, Value<V, TS>>,
}

impl<K, V> std::fmt::Debug for TimedMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TimedMap")
    }
}

impl<K: Eq + Hash, V> TimedMap<K, V> {
    /// Create a new instance of [`TimedMap`] with the default
    /// [`TimeSource`] implementation [`Instant`].
    pub fn new() -> Self {
        Self::new_with_timesource()
    }
}

impl<K: Eq + Hash, V, TS> TimedMap<K, V, TS> {
    /// Create a new instance of [`TimedMap`] with a custom
    /// [`TimeSource`] implementation.
    pub fn new_with_timesource() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }
}

impl<K, V, TS> TimedMap<K, V, TS>
where
    K: Eq + PartialEq + Hash + Clone,
    V: Clone,
    TS: TimeSource,
{
    /// Add a new key-value pair to the map with the
    /// given lifetime.
    ///
    /// When the lifetime has passed, the key-value pair
    /// will be no more accessible.
    ///
    /// # Example
    /// ```
    /// use timedmap::TimedMap;
    /// use std::time::Duration;
    ///
    /// let tm = TimedMap::new();
    /// tm.insert("foo", "bar", Duration::from_millis(10));
    /// assert_eq!(tm.get(&"foo"), Some("bar"));
    ///
    /// std::thread::sleep(Duration::from_millis(20));
    /// assert_eq!(tm.get(&"foo"), None);
    /// ```
    pub fn insert(&self, key: K, value: V, lifetime: Duration) {
        self.inner.insert(key, Value::new(value, lifetime));
    }

    /// Returns a copy of the value corresponding to the
    /// given key.
    ///
    /// [`None`] is returned when the values lifetime has
    /// been passed.
    ///
    /// # Behavior
    ///
    /// If the key-value pair has expired and not been
    /// cleaned up before, it will be removed from the
    /// map on next retrival try.
    pub fn get(&self, key: &K) -> Option<V> {
        self.get_value(key).map(|v| v.value())
    }

    /// Returns `true` when the map contains a non-expired
    /// value for the given key.
    ///
    /// # Behavior
    ///
    /// Because this method is basically a shorthand for
    /// [get(key).is_some()](#method.get), it behaves the
    /// same on retrival of expired pairs.
    pub fn contains(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// Removes the given key-value pair from the map and
    /// returns the value if it was previously in the map
    /// and is not expired.
    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.inner.remove(key).and_then(|(_, v)| v.value_checked())
    }

    /// Sets the lifetime of the value coresponding to the
    /// given key to the new lifetime from now.
    ///
    /// Returns `true` if a non-expired value exists for the
    /// given key.
    pub fn refresh(&self, key: &K, new_lifetime: Duration) -> bool {
        let Some(mut v) = self.get_value(key) else {
            return false;
        };

        v.set_expiry(new_lifetime);
        self.inner.insert(key.clone(), v);

        true
    }

    /// Extends the lifetime of the value coresponding to the
    /// given key to the new lifetime from now.
    ///
    /// Returns `true` if a non-expired value exists for the
    /// given key.
    pub fn extend(&self, key: &K, added_lifetime: Duration) -> bool {
        let Some(mut v) = self.get_value(key) else {
            return false;
        };

        v.add_expiry(added_lifetime);
        self.inner.insert(key.clone(), v);

        true
    }

    /// Returns the number of key-value pairs in the map
    /// which have not been expired.
    pub fn len(&self) -> usize {
        self.inner.iter().filter(|v| !v.is_expired()).count()
    }

    /// Returns `true` when the map does not contain any
    /// non-expired key-value pair.
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    /// Clears the map, removing all key-value pairs.
    pub fn clear(&self) {
        self.inner.clear();
    }

    /// Create a snapshot of the current state of the maps
    /// key-value entries.
    ///
    /// It does only contain all non-expired key-value pairs.
    pub fn snapshot<B: FromIterator<(K, V)>>(&self) -> B {
        self.inner
            .iter()
            .filter(|v| !v.is_expired())
            .map(|kv| (kv.key().clone(), kv.value().value()))
            .collect()
    }

    /// Retrieves the raw [`Value`] wrapper by the given key if
    /// the key-value pair has not been expired yet.
    ///
    /// If the given key-value pair is expired and not cleaned
    /// up yet, it will be removed from the map automatically.
    pub fn get_value<Q>(&self, key: &Q) -> Option<Value<V, TS>>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let v = self.get_value_unchecked(key);
        let v = v?;
        if v.is_expired() {
            self.remove(key);
            return None;
        }
        Some(v)
    }

    /// Retrieves the raw [`Value`] wrapper by the given key
    /// without checking expiry.
    pub fn get_value_unchecked<Q>(&self, key: &Q) -> Option<Value<V, TS>>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.inner.get(key).map(|x| x.value().clone())
    }
}

impl<K, V, TS> Cleanup for TimedMap<K, V, TS>
where
    K: Eq + PartialEq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
    TS: TimeSource + Send + Sync,
{
    fn cleanup(&self) {
        let now = TS::now();

        let mut keys = vec![];
        {
            keys.extend(
                self.inner
                    .iter()
                    .filter(|val| val.is_expired_at(&now))
                    .map(|kv| kv.key().clone()),
            );
        }

        if keys.is_empty() {
            return;
        }

        for key in keys {
            self.inner.remove(&key);
        }

        // TODO: Maybe shrink the map down if it exceeds a predefined
        // capacity, like
        // if m.capacity() > SOME_CAP_VAL {
        //     m.shrink_to_fit();
        // }
    }
}

impl<K: Eq + Hash, V> Default for TimedMap<K, V> {
    fn default() -> Self {
        Self {
            inner: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock_instant::{Instant, MockClock};

    #[test]
    fn get_checked() {
        let tm: TimedMap<_, _, Instant> = TimedMap::new_with_timesource();
        assert_eq!(tm.len(), 0);
        assert!(tm.is_empty());

        tm.insert("a", "b", Duration::from_millis(10));
        assert_eq!(tm.len(), 1);
        assert!(!tm.is_empty());

        let v = tm.get(&"x");
        assert_eq!(v, None);
        assert!(!tm.contains(&"x"));

        MockClock::advance(Duration::from_millis(5));
        let v = tm.get(&"a");
        assert_eq!(v, Some("b"));
        assert!(tm.contains(&"a"));
        assert_eq!(tm.len(), 1);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(6));
        let v = tm.get(&"a");
        assert_eq!(v, None);
        assert!(!tm.contains(&"a"));
        assert_eq!(tm.len(), 0);
        assert!(tm.is_empty());
    }

    #[test]
    fn strings() {
        let tm: TimedMap<_, _> = TimedMap::new();
        let s = String::from("foo");
        tm.insert(s.clone(), "bar", Duration::from_secs(1));
        tm.get_value(&s);
        tm.contains(&s);
        tm.extend(&s, Duration::from_secs(1));
        tm.get(&s);
        tm.remove(&s);
    }

    #[test]
    fn remove() {
        let tm: TimedMap<_, _, Instant> = TimedMap::new_with_timesource();
        tm.insert("a", 1, Duration::from_millis(100));
        tm.insert("b", 2, Duration::from_millis(100));
        assert_eq!(tm.len(), 2);
        assert!(!tm.is_empty());

        let v = tm.remove(&"a");
        assert_eq!(v, Some(1));
        assert!(!tm.contains(&"a"));
        assert_eq!(tm.get(&"b"), Some(2));
        assert!(tm.contains(&"b"));
        assert_eq!(tm.len(), 1);
        assert!(!tm.is_empty());

        let v = tm.remove(&"a");
        assert_eq!(v, None);
        assert!(!tm.contains(&"a"));
        assert_eq!(tm.get(&"b"), Some(2));
        assert!(tm.contains(&"b"));
        assert_eq!(tm.len(), 1);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(120));
        let v = tm.remove(&"b");
        assert_eq!(v, None);
        assert!(!tm.contains(&"b"));
        assert_eq!(tm.len(), 0);
        assert!(tm.is_empty());
    }

    #[test]
    fn refresh() {
        let tm: TimedMap<_, _, Instant> = TimedMap::new_with_timesource();
        tm.insert("a", 1, Duration::from_millis(100));
        tm.insert("b", 2, Duration::from_millis(100));
        assert_eq!(tm.len(), 2);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(60));
        assert_eq!(tm.get(&"a"), Some(1));
        assert_eq!(tm.get(&"b"), Some(2));
        assert_eq!(tm.len(), 2);
        assert!(!tm.is_empty());

        assert!(tm.refresh(&"b", Duration::from_millis(60)));
        assert!(!tm.refresh(&"c", Duration::from_millis(60)));

        MockClock::advance(Duration::from_millis(50));
        assert_eq!(tm.get(&"a"), None);
        assert_eq!(tm.get(&"b"), Some(2));
        assert_eq!(tm.len(), 1);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(50));
        assert_eq!(tm.get(&"a"), None);
        assert_eq!(tm.get(&"b"), None);
        assert_eq!(tm.len(), 0);
        assert!(tm.is_empty());
    }

    #[test]
    fn extend() {
        let tm: TimedMap<_, _, Instant> = TimedMap::new_with_timesource();
        tm.insert("a", 1, Duration::from_millis(100));
        tm.insert("b", 2, Duration::from_millis(100));
        assert_eq!(tm.len(), 2);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(60));
        assert_eq!(tm.get(&"a"), Some(1));
        assert_eq!(tm.get(&"b"), Some(2));
        assert_eq!(tm.len(), 2);
        assert!(!tm.is_empty());

        assert!(tm.extend(&"b", Duration::from_millis(10)));
        assert!(!tm.extend(&"c", Duration::from_millis(10)));

        MockClock::advance(Duration::from_millis(50));
        assert_eq!(tm.get(&"a"), None);
        assert_eq!(tm.get(&"b"), Some(2));
        assert_eq!(tm.len(), 1);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(50));
        assert_eq!(tm.get(&"a"), None);
        assert_eq!(tm.get(&"b"), None);
        assert_eq!(tm.len(), 0);
        assert!(tm.is_empty());
    }

    #[test]
    fn cleanup() {
        let tm: TimedMap<_, _, Instant> = TimedMap::new_with_timesource();

        tm.insert("a", 1, Duration::from_millis(5));
        tm.insert("b", 2, Duration::from_millis(10));
        tm.insert("c", 3, Duration::from_millis(15));
        assert_eq!(tm.len(), 3);
        assert!(!tm.is_empty());

        tm.cleanup();
        assert!(tm.contains(&"a"));
        assert!(tm.contains(&"b"));
        assert!(tm.contains(&"c"));
        assert_eq!(tm.len(), 3);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(6));
        tm.cleanup();
        assert!(!tm.contains(&"a"));
        assert!(tm.contains(&"b"));
        assert!(tm.contains(&"c"));
        assert_eq!(tm.len(), 2);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(5));
        tm.cleanup();
        assert!(!tm.contains(&"a"));
        assert!(!tm.contains(&"b"));
        assert!(tm.contains(&"c"));
        assert_eq!(tm.len(), 1);
        assert!(!tm.is_empty());

        MockClock::advance(Duration::from_millis(5));
        tm.cleanup();
        assert!(!tm.contains(&"a"));
        assert!(!tm.contains(&"b"));
        assert!(!tm.contains(&"c"));
        assert_eq!(tm.len(), 0);
        assert!(tm.is_empty());
    }

    #[test]
    fn clear() {
        let tm: TimedMap<_, _, Instant> = TimedMap::new_with_timesource();

        tm.insert("a", 1, Duration::from_millis(5));
        tm.insert("b", 2, Duration::from_millis(10));
        tm.insert("c", 3, Duration::from_millis(15));
        assert_eq!(tm.len(), 3);
        assert!(!tm.is_empty());

        tm.clear();

        assert!(!tm.contains(&"a"));
        assert!(!tm.contains(&"b"));
        assert!(!tm.contains(&"c"));
        assert_eq!(tm.len(), 0);
        assert!(tm.is_empty());
    }
}
