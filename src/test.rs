use std::collections::HashMap;
use crate::cache_api::{LoadingCache, CacheLoadingError};
#[cfg(feature = "ttl-cache")]
use crate::cache_api::WithMeta;
use tokio::time::Duration;
use cache_loader_async_macros::test_with_features;
use crate::backing::HashMapBacking;
#[cfg(feature = "ttl-cache")]
use crate::backing::TtlMeta;
#[cfg(feature = "lru-cache")]
use crate::backing::LruCacheBacking;
#[cfg(feature = "ttl-cache")]
use crate::backing::TtlCacheBacking;

#[derive(Debug, Clone)]
pub struct ThingOne(u8);

#[derive(Debug, Clone)]
pub struct ThingTwo(String);


#[tokio::test]
async fn test_load() {
    let thing_one_static_db: HashMap<String, ThingOne> =
        vec![("foo".into(), ThingOne(32)), ("bar".into(), ThingOne(64))]
            .into_iter()
            .collect();

    let thing_two_static_db: HashMap<String, ThingTwo> = vec![
        ("fizz".into(), ThingTwo("buzz".into())),
        ("coca".into(), ThingTwo("cola".into())),
    ]
        .into_iter()
        .collect();


    let cache_one = LoadingCache::new(move |key: String| {
        let db_clone = thing_one_static_db.clone();
        async move {
            db_clone.get(&key).cloned().ok_or(1)
        }
    });

    let cache_two = LoadingCache::new(move |key: String| {
        let db_clone = thing_two_static_db.clone();
        async move {
            db_clone.get(&key).cloned().ok_or(1)
        }
    });

    let result_one = cache_one.get("foo".to_owned()).await.unwrap().0;
    let result_two = cache_two.get("fizz".to_owned()).await.unwrap().0;

    println!("test_load one: {}", result_one);
    println!("test_load two: {}", result_two);

    assert_eq!(result_one, 32);
    assert_eq!(result_two, "buzz".to_owned());
}

#[cfg(feature = "ttl-cache")]
#[tokio::test]
async fn test_ttl_load_meta() {
    let cache: LoadingCache<String, String, u8, _> =
        LoadingCache::with_meta_loader(TtlCacheBacking::new(Duration::from_secs(1)), move |key: String| {
        async move {
            if key.len() < 5 {
                Ok(key.to_lowercase())
                    .with_meta(Some(TtlMeta::from(Duration::from_secs(5))))
            } else {
                Ok(key.to_lowercase())
                    .with_meta(None)
            }
        }
    });

    assert_eq!(cache.get("a".to_owned()).await.unwrap(), "a".to_owned());
    assert_eq!(cache.get("bbbbb".to_owned()).await.unwrap(), "bbbbb".to_owned());

    tokio::time::sleep(Duration::from_secs(2)).await;

    assert!(cache.exists("a".to_owned()).await.unwrap());
    assert!(!cache.exists("bbbbb".to_owned()).await.unwrap());
}

test_with_features! {
    test_write cache <String, String, u8> {
        Ok(key.to_lowercase())
    }

    let lowercase_result = cache.get("LOL".to_owned()).await.unwrap();
    println!("Result of lowercase loader: {}", lowercase_result);
    assert_eq!(lowercase_result, "lol".to_owned());

    cache.set("test".to_owned(), "BIG".to_owned()).await.ok();

    let result = cache.get("test".to_owned()).await.unwrap();
    println!("Result of lowercase loader with manual set: {}", result);
    assert_eq!(result, "BIG".to_owned());
}

test_with_features! {
    get_if_present cache <String, String, u8> {
        Ok(key.to_lowercase())
    }

    let option = cache.get_if_present("test".to_owned()).await.unwrap();
    assert!(option.is_none());

    cache.set("test".to_owned(), "ok".to_owned()).await.ok();

    let option = cache.get_if_present("test".to_owned()).await.unwrap();
    assert!(option.is_some());
}

test_with_features! {
    exists cache <String, String, u8> {
        Ok(key.to_lowercase())
    }

    let exists = cache.exists("test".to_owned()).await.unwrap();
    assert!(!exists);

    cache.set("test".to_owned(), "ok".to_owned()).await.ok();

    let exists = cache.exists("test".to_owned()).await.unwrap();
    assert!(exists);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_update() {
    let cache: LoadingCache<String, String, u8, HashMapBacking<_, _>> = LoadingCache::new(move |key: String| {
        async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(key.to_lowercase())
        }
    });

    // We test to update an existing key
    cache.set("woob".to_owned(), "Woob".to_owned()).await.ok();
    let result = cache.update("woob".to_owned(), |value| {
        let mut value = value.clone();
        value.push_str("Woob");
        value
    }).await.unwrap();

    println!("Result of updating existing key woob -> Woob with append Woob: {}", result);
    assert_eq!(result, "WoobWoob".to_owned());


    // We test to update an loaded key
    let result = cache.update("TEST".to_owned(), |value| {
        let mut value = value.clone();
        value.push_str("_magic");
        value
    }).await.unwrap();

    println!("Result of updating loaded key test -> test with append _magic: {}", result);
    assert_eq!(result, "test_magic".to_owned());

    // We test to update an loaded key which is `set` during the load time
    // Our to_lower_case cache is supposed to load the `monka` key as `monka` value
    // yet we'll be setting `race` as value while the loader function is running
    // we'll expect the `race` value to be the preceding value over `monka`
    // as it is user-controlled and more up-to-date.
    // so the result of our test is supposed to be `race_condition`
    let inner_cache = cache.clone();
    let handle = tokio::spawn(async move {
        inner_cache.update("monka".to_owned(), |value| {
            let mut value = value.clone();
            value.push_str("_condition");
            value
        }).await.unwrap()
    });
    let inner_cache = cache.clone();
    tokio::spawn(async move {
        inner_cache.set("monka".to_owned(), "race".to_owned()).await.ok();
    });
    let result = handle.await.unwrap();
    println!("Result of updating loaded key while setting key manually with append _condition: {}", result);
    assert_eq!(result, "race_condition".to_owned());
}

test_with_features! {
    update_if_exists cache <String, String, u8> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(key.to_lowercase())
    }
    cache.set("test".to_owned(), "test".to_owned()).await.ok();

    let no_value = cache.update_if_exists("test2".to_owned(), |val| {
        let mut clone = val.clone();
        clone.push_str("test");
        clone
    }).await.unwrap();

    let two_test = cache.update_if_exists("test".to_owned(), |val| {
        let mut clone = val.clone();
        clone.push_str("test");
        clone
    }).await.unwrap();

    assert!(no_value.is_none());
    assert!(two_test.is_some());
    // returns updated value!
    assert_eq!(two_test.unwrap(), "testtest");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_update_mut() {
    let cache: LoadingCache<String, String, u8, HashMapBacking<_, _>> = LoadingCache::new(move |key: String| {
        async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(key.to_lowercase())
        }
    });

    // We test to update an existing key
    cache.set("woob".to_owned(), "Woob".to_owned()).await.ok();
    let result = cache.update_mut("woob".to_owned(), |value| {
        value.push_str("Woob");
    }).await.unwrap();

    println!("Result of updating existing key woob -> Woob with append Woob: {}", result);
    assert_eq!(result, "WoobWoob".to_owned());


    // We test to update an loaded key
    let result = cache.update_mut("TEST".to_owned(), |value| {
        value.push_str("_magic");
    }).await.unwrap();

    println!("Result of updating loaded key test -> test with append _magic: {}", result);
    assert_eq!(result, "test_magic".to_owned());

    // We test to update an loaded key which is `set` during the load time
    // Our to_lower_case cache is supposed to load the `monka` key as `monka` value
    // yet we'll be setting `race` as value while the loader function is running
    // we'll expect the `race` value to be the preceding value over `monka`
    // as it is user-controlled and more up-to-date.
    // so the result of our test is supposed to be `race_condition`
    let inner_cache = cache.clone();
    let handle = tokio::spawn(async move {
        inner_cache.update_mut("monka".to_owned(), |value| {
            value.push_str("_condition");
        }).await.unwrap()
    });
    let inner_cache = cache.clone();
    tokio::spawn(async move {
        inner_cache.set("monka".to_owned(), "race".to_owned()).await.ok();
    });
    let result = handle.await.unwrap();
    println!("Result of updating loaded key while setting key manually with append _condition: {}", result);
    assert_eq!(result, "race_condition".to_owned());
}

test_with_features! {
    update_mut_if_exists cache <String, String, u8> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(key.to_lowercase())
    }
    cache.set("test".to_owned(), "test".to_owned()).await.ok();

    let no_value = cache.update_mut_if_exists("test2".to_owned(), |val| {
        val.push_str("test");
    }).await.unwrap();

    let two_test = cache.update_mut_if_exists("test".to_owned(), |val| {
        val.push_str("test");
    }).await.unwrap();

    assert!(no_value.is_none());
    assert!(two_test.is_some());
    // returns updated value!
    assert_eq!(two_test.unwrap(), "testtest");
}

test_with_features! {
    remove cache <String, String, u8> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(key.to_lowercase())
    }

    cache.set("test".to_owned(), "lol".to_owned()).await.ok();

    assert_eq!(cache.get("test".to_owned()).await.unwrap(), "lol".to_owned());

    cache.remove("test".to_owned()).await.ok();

    assert_eq!(cache.get("test".to_owned()).await.unwrap(), "test".to_owned());
}


test_with_features! {
    remove_if cache <u64, u64, u8> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(key * 2)
    }

    cache.set(1, 2).await.ok();
    cache.set(2, 4).await.ok();
    cache.set(3, 6).await.ok();
    cache.set(4, 8).await.ok();
    cache.set(5, 10).await.ok();

    assert!(cache.get_if_present(1).await.unwrap().is_some());
    assert!(cache.get_if_present(2).await.unwrap().is_some());
    assert!(cache.get_if_present(3).await.unwrap().is_some());
    assert!(cache.get_if_present(4).await.unwrap().is_some());
    assert!(cache.get_if_present(5).await.unwrap().is_some());

    cache.remove_if(|(k, _)| k > &3).await.unwrap();

    assert!(cache.get_if_present(1).await.unwrap().is_some());
    assert!(cache.get_if_present(2).await.unwrap().is_some());
    assert!(cache.get_if_present(3).await.unwrap().is_some());
    // next two should be none after remove
    assert!(cache.get_if_present(4).await.unwrap().is_none());
    assert!(cache.get_if_present(5).await.unwrap().is_none());
}


test_with_features! {
    load_error cache <String, String, u8> {
        Err(5)
    }

    let cache_loading_error = cache.get("test".to_owned()).await.expect_err("Didn't error, what?");
    if let CacheLoadingError::LoadingError(val) = cache_loading_error {
        assert_eq!(val, 5)
    } else {
        panic!("Unexpected error type");
    }
}

test_with_features! {
    meta cache <String, String, u8> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(key.to_lowercase())
    }

    let meta = cache.get_with_meta("key".to_owned()).await.unwrap();
    assert!(!meta.cached);

    let meta = cache.get_with_meta("key".to_owned()).await.unwrap();
    assert!(meta.cached);
}

#[cfg(feature = "lru-cache")]
#[tokio::test]
async fn test_lru_backing() {
    let cache: LoadingCache<_, _, u8, _> = LoadingCache::with_backing(LruCacheBacking::new(2), move |key: String| {
        async move {
            Ok(key.to_lowercase())
        }
    });

    cache.set("key1".to_owned(), "value1".to_lowercase()).await.ok();
    tokio::time::sleep(Duration::from_secs(1)).await;
    cache.set("key2".to_owned(), "value2".to_lowercase()).await.ok();
    tokio::time::sleep(Duration::from_secs(1)).await;
    // cache is full

    assert_eq!(cache.get("key1".to_owned()).await.unwrap(), "value1".to_lowercase());

    // we reused key1, key1 is more recent than key 2
    tokio::time::sleep(Duration::from_secs(1)).await;
    cache.set("key3".to_owned(), "value3".to_lowercase()).await.ok();

    assert_eq!(cache.get("key1".to_owned()).await.unwrap(), "value1".to_lowercase());
    assert_eq!(cache.get("key3".to_owned()).await.unwrap(), "value3".to_lowercase());
    assert_eq!(cache.get("key2".to_owned()).await.unwrap(), "key2".to_lowercase());

    cache.set("remove_test".to_owned(), "delete_me".to_lowercase()).await.ok();
    cache.remove("remove_test".to_owned()).await.ok();
    assert_eq!(cache.get("remove_test".to_owned()).await.unwrap(), "remove_test".to_lowercase());
}

#[cfg(feature = "ttl-cache")]
#[tokio::test]
async fn test_ttl_backing() {
    let cache: LoadingCache<_, _, u8, _> = LoadingCache::with_backing(
        TtlCacheBacking::new(Duration::from_secs(3)), move |key: String| {
            async move {
                Ok(key.to_lowercase())
            }
        });

    cache.set("key1".to_owned(), "value1".to_lowercase()).await.ok();
    tokio::time::sleep(Duration::from_secs(2)).await;
    cache.set("key2".to_owned(), "value2".to_lowercase()).await.ok();

    assert_eq!(cache.get("key1".to_owned()).await.unwrap(), "value1".to_lowercase());
    assert_eq!(cache.get("key2".to_owned()).await.unwrap(), "value2".to_lowercase());

    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(cache.exists("key1".to_owned()).await.unwrap(), false);
}