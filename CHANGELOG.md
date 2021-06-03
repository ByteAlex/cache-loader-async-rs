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