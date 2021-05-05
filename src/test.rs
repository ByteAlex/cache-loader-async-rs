use std::collections::HashMap;
use crate::cache_api::LoadingCache;
use tokio::time::Duration;

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


    let (cache_one, _) = LoadingCache::new(move |key: String| {
        let db_clone = thing_one_static_db.clone();
        async move {
            db_clone.get(&key).cloned()
        }
    });

    let (cache_two, _) = LoadingCache::new(move |key: String| {
        let db_clone = thing_two_static_db.clone();
        async move {
            db_clone.get(&key).cloned()
        }
    });

    let result_one = cache_one.get("foo".to_owned()).await.unwrap().0;
    let result_two = cache_two.get("fizz".to_owned()).await.unwrap().0;

    println!("test_load one: {}", result_one);
    println!("test_load two: {}", result_two);

    assert_eq!(result_one, 32);
    assert_eq!(result_two, "buzz".to_owned());
}

#[tokio::test]
async fn test_write() {
    let (cache, _) = LoadingCache::new(move |key: String| {
        async move {
            Some(key.to_lowercase())
        }
    });

    let lowercase_result = cache.get("LOL".to_owned()).await.unwrap();
    println!("Result of lowercase loader: {}", lowercase_result);
    assert_eq!(lowercase_result, "lol".to_owned());

    cache.set("test".to_owned(), "BIG".to_owned()).await.ok();

    let result = cache.get("test".to_owned()).await.unwrap();
    println!("Result of lowercase loader with manual set: {}", result);
    assert_eq!(result, "BIG".to_owned());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_update() {
    let (cache, _) = LoadingCache::new(move |key: String| {
        async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Some(key.to_lowercase())
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