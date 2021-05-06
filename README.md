# cache-loader-async
[crates.io](https://crates.io/crates/cache_loader_async)

The goal of this crate is to provide a thread-safe and easy way to access any data structure
which might is stored in a database at most once and keep it in cache for further requests.

This library is based on [tokio-rs](https://github.com/tokio-rs/tokio) and 
[futures](https://github.com/rust-lang/futures-rs).

# Usage
Using this library is as easy as that:
```rust
#[tokio::main]
async fn main() {
    let static_db: HashMap<String, u32> =
        vec![("foo".into(), 32), ("bar".into(), 64)]
            .into_iter()
            .collect();
    
    let (cache, _) = LoadingCache::new(move |key: String| {
        let db_clone = static_db.clone();
        async move {
            db_clone.get(&key).cloned()
        }
    });

    let result = cache.get("foo".to_owned()).await.unwrap();

    assert_eq!(result, 32);
}
```

The LoadingCache will first try to look up the result in an internal HashMap and if it's
not found and there's no load ongoing, it will fire the load request and queue any other
get requests until the load request finishes.