# v0.2.0
New/Changed:
* TTL Backing now uses fewer iterations through the entire list
* Backings can define a `Meta` type which can be used for improved handling with
the loader
* TTL Backing now supports a `TtlMeta` -> Custom duration per entry
* TTL Backing now supports other nested `CacheBacking`s (i.e. TTL+LRU)
* New methods: `LoadingCache::with_meta_loader()`, `LoadingCache#set_with_meta`

Breaking:
* Backings trait changed:
  1. All methods now return a result
  2. contains_key takes a mutable self reference
  3. remove_if returns the removed k/v pairs
  4. Meta Type added / set signature now has a meta field

# v0.1.2
New/Changed:
* Replace SystemTime calls with Instant
* Implement additional helper method: clear
* Change new/with_backing signature to return no CacheHandle

Breaking: 
* Instantiation method signature changed
* Backing interface extended with "clear method"

# v0.1.1
* Add additional helper methods
  - update_if_exists
  - update_mut_if_exists
  - remove_if
* Change Backing Trait to support `remove_if`. This might be breaking if you have a custom backing.

# v0.1.0
* Add ttl-cache feature
* \[Breaking] Return type of the loader function changed from Optional<T> to Result<T, E>
* \[Breaking] CacheLoadingError now contains the LoadingError(E) and various other error types instead of a simple struct
* Additional method to receive additional data about the source of the data - Either cache or loader function

# v0.0.5
* Add update_mut function to update a mutable entry
* Update tokio-rs

# v0.0.4
* Implement update function
* All api methods should have some documentation now
* Start writing changelogs :^)