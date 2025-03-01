use log::{debug, error, info};
use lru::LruCache;
use matchit::Router;
use proxy_wasm::traits::*;
use proxy_wasm::types::*;
use serde_json::Value;
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::rc::Rc;

static DEFAULT_CACHE_SIZE: usize = 1024;

proxy_wasm::main! {{
    proxy_wasm::set_log_level(LogLevel::Trace);
    proxy_wasm::set_root_context(|_| -> Box<dyn RootContext> {
        Box::new(OpenapiPathRoot::new())
    });
}}

struct OpenapiPathRoot {
    router: Rc<Router<String>>,
    cache: Option<Rc<RefCell<LruCache<String, String>>>>,
}

impl OpenapiPathRoot {
    fn new() -> Self {
        Self {
            router: Rc::new(Router::new()),
            cache: Some(Rc::new(RefCell::new(LruCache::new(
                NonZeroUsize::new(DEFAULT_CACHE_SIZE).unwrap(),
            )))),
        }
    }
}

impl Context for OpenapiPathRoot {
    fn on_done(&mut self) -> bool {
        info!("[opf] openapi-path-filter terminated");
        true
    }
}

impl RootContext for OpenapiPathRoot {
    fn on_vm_start(&mut self, _vm_configuration_size: usize) -> bool {
        info!("[opf] openapi-path-filter initialized");
        true
    }

    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }

    fn on_configure(&mut self, _: usize) -> bool {
        debug!("[opf] Configuring openapi-path-filter");
        let config_bytes = match self.get_plugin_configuration() {
            Some(bytes) => bytes,
            None => {
                error!("[opf] No plugin configuration found");
                return false;
            }
        };

        let config_str = match String::from_utf8(config_bytes) {
            Ok(s) => s,
            Err(e) => {
                error!("[opf] Failed to convert bytes to UTF-8 string: {}", e);
                return false;
            }
        };

        let config: Value = match serde_json::from_str(&config_str) {
            Ok(v) => v,
            Err(e) => {
                error!("[opf] Failed to parse JSON configuration: {}", e);
                return false;
            }
        };

        match self.configure(&config) {
            Ok(_) => {
                info!("[opf] Configuration successful");
                true
            }
            Err(e) => {
                error!("[opf] Configuration failed: {}", e);
                false
            }
        }
    }

    fn create_http_context(&self, _: u32) -> Option<Box<dyn HttpContext>> {
        debug!("[opf] Creating HTTP context");
        Some(Box::new(OpenapiPathFilter {
            router: Rc::clone(&self.router),
            cache: self.cache.as_ref().map(Rc::clone),
        }))
    }
}

impl OpenapiPathRoot {
    fn configure(&mut self, config: &Value) -> Result<(), Box<dyn std::error::Error>> {
        let size = config
            .get("cache_size")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_CACHE_SIZE as u64);

        if size == 0 {
            self.cache = None;
        } else {
            let cache_size = NonZeroUsize::new(size as usize)
                .unwrap_or(NonZeroUsize::new(DEFAULT_CACHE_SIZE).unwrap());
            self.cache = Some(Rc::new(RefCell::new(LruCache::new(cache_size))));
        }

        let paths = config
            .get("paths")
            .and_then(Value::as_object)
            .ok_or("Invalid or missing 'paths' in configuration")?;

        let mut new_router = Router::new();
        for (path, _) in paths {
            debug!("[opf] Inserting route: {}", path);
            new_router.insert(path, path.clone())?;
        }
        self.router = Rc::new(new_router);

        info!(
            "[opf] Router configured successfully with {} paths and cache size {}",
            paths.len(),
            match &self.cache {
                Some(cache) => cache.borrow().cap().to_string(),
                None => "None".to_string(),
            }
        );
        Ok(())
    }
}

struct OpenapiPathFilter {
    router: Rc<Router<String>>,
    cache: Option<Rc<RefCell<LruCache<String, String>>>>,
}

impl Context for OpenapiPathFilter {}

impl HttpContext for OpenapiPathFilter {
    fn on_http_request_headers(&mut self, _nheaders: usize, _end_of_stream: bool) -> Action {
        debug!("[opf] Getting the path from header");
        let path = self.get_http_request_header(":path").unwrap_or_default();
        if let Some(matched_value) = self.get_openapi_path(&path) {
            self.set_http_request_header("x-openapi-path", Some(&matched_value));
        }
        Action::Continue
    }
}

impl OpenapiPathFilter {
    fn get_openapi_path(&self, path: &str) -> Option<String> {
        let normalized_path = path.split('?').next().unwrap_or("").to_string();
        if let Some(cache) = &self.cache {
            let mut cache = cache.borrow_mut();
            if let Some(matched_value) = cache.get(&normalized_path) {
                debug!(
                    "[opf] Cache hit for path: {}, value: {}",
                    normalized_path, matched_value
                );
                return Some(matched_value.clone());
            }
        }
        debug!(
            "[opf] Cache miss or cache disabled for path: {}",
            normalized_path
        );
        match self.router.at(&normalized_path) {
            Ok(matched) => {
                let matched_value = matched.value.clone();
                if let Some(cache) = &self.cache {
                    cache
                        .borrow_mut()
                        .put(normalized_path, matched_value.clone());
                }
                Some(matched_value)
            }
            Err(_) => {
                debug!("[opf] No match found for path: {}", normalized_path);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEST_CONFIG: &str = r#"{
        "cache_size": 1024,
        "paths": {
            "/api/users/{id}": {},
            "/api/posts/{postId}/comments/{commentId}": {},
            "/api/items": {}
        }
    }"#;

    #[test]
    fn test_path_parameter_matching() {
        let mut root_ctx = OpenapiPathRoot::new();
        root_ctx
            .configure(&serde_json::from_str(TEST_CONFIG).unwrap())
            .unwrap();

        let http_ctx = OpenapiPathFilter {
            router: Rc::clone(&root_ctx.router),
            cache: root_ctx.cache.as_ref().map(Rc::clone),
        };

        let test_cases = vec![
            ("/api/items", Some("/api/items".to_string())),
            ("/api/users/123", Some("/api/users/{id}".to_string())),
            (
                "/api/posts/456/comments/789",
                Some("/api/posts/{postId}/comments/{commentId}".to_string()),
            ),
            (
                "/api/users/123?key=value",
                Some("/api/users/{id}".to_string()),
            ),
            ("/api/items?sort=asc&page=2", Some("/api/items".to_string())),
            (
                "/api/posts/456/comments/789?active=true",
                Some("/api/posts/{postId}/comments/{commentId}".to_string()),
            ),
            (
                "/api/items?filter=active&limit=10",
                Some("/api/items".to_string()),
            ),
            (
                "/api/posts/1/comments/2?a=b&c=d",
                Some("/api/posts/{postId}/comments/{commentId}".to_string()),
            ),
            (
                "/api/users/456?key=value&nested[key]=val",
                Some("/api/users/{id}".to_string()),
            ),
            ("/api/users/123/", None),
            ("/api/items/", None),
            ("/api/posts/456/comments/789/", None),
            ("/api/users/123/?key=value", None),
            ("/api/items/?sort=asc", None),
            ("/api/users", None),
            ("/api/posts/456", None),
            ("/api/nonexistent", None),
        ];

        for (input_path, expected) in test_cases {
            let result = http_ctx.get_openapi_path(input_path);
            assert_eq!(
                result, expected,
                "Path '{}' should match '{:?}' but got '{:?}'",
                input_path, expected, result
            );
        }
    }

    #[test]
    fn test_cache_behavior() {
        let mut root_ctx = OpenapiPathRoot::new();
        let config = json!({
            "cache_size": 2,
            "paths": {
                "/api/users/{id}": {},
                "/api/items": {}
            }
        })
        .to_string();
        root_ctx
            .configure(&serde_json::from_str(&config).unwrap())
            .unwrap();

        let http_ctx1 = OpenapiPathFilter {
            router: Rc::clone(&root_ctx.router),
            cache: root_ctx.cache.as_ref().map(Rc::clone),
        };

        let http_ctx2 = OpenapiPathFilter {
            router: Rc::clone(&root_ctx.router),
            cache: root_ctx.cache.as_ref().map(Rc::clone),
        };

        let path1 = "/api/users/123";
        let result1 = http_ctx1.get_openapi_path(path1);
        assert_eq!(result1, Some("/api/users/{id}".to_string()));

        let result2 = http_ctx2.get_openapi_path(path1);
        assert_eq!(result2, Some("/api/users/{id}".to_string()));

        if let Some(cache) = &root_ctx.cache {
            assert_eq!(cache.borrow().len(), 1);
        } else {
            panic!("Cache should be enabled");
        }

        let path2 = "/api/items";
        let result3 = http_ctx1.get_openapi_path(path2);
        assert_eq!(result3, Some("/api/items".to_string()));

        if let Some(cache) = &root_ctx.cache {
            assert_eq!(cache.borrow().len(), 2);
        } else {
            panic!("Cache should be enabled");
        }

        let path3 = "/api/users/456";
        let result4 = http_ctx2.get_openapi_path(path3);
        assert_eq!(result4, Some("/api/users/{id}".to_string()));

        if let Some(cache) = &root_ctx.cache {
            let cache = cache.borrow();
            assert!(cache.contains(&path2.to_string()));
            assert!(cache.contains(&path3.to_string()));
            assert!(!cache.contains(&path1.to_string()));
        } else {
            panic!("Cache should be enabled");
        }
    }

    #[test]
    fn test_cache_size_parsing() {
        let paths = json!({
            "/api/users/{id}": {},
        });
        let test_cases = vec![
            // Missing cache_size
            (json!({ "paths": paths.clone() }), Some(DEFAULT_CACHE_SIZE)),
            // Not a number
            (
                json!({ "cache_size": "not a number", "paths": paths.clone() }),
                Some(DEFAULT_CACHE_SIZE),
            ),
            // Negative number (treated as not u64)
            (
                json!({ "cache_size": -1, "paths": paths.clone() }),
                Some(DEFAULT_CACHE_SIZE),
            ),
            // Zero (cache disabled)
            (json!({ "cache_size": 0, "paths": paths.clone() }), None),
            // Positive number
            (
                json!({ "cache_size": 100, "paths": paths.clone() }),
                Some(100),
            ),
        ];

        for (config, expected_cache) in test_cases {
            let mut root_ctx = OpenapiPathRoot::new();
            root_ctx.configure(&config).unwrap();
            match expected_cache {
                Some(size) => {
                    assert!(
                        root_ctx.cache.is_some(),
                        "Cache should be enabled for config: {}",
                        config
                    );
                    let cache = root_ctx.cache.as_ref().unwrap().borrow();
                    assert_eq!(
                        cache.cap(),
                        NonZeroUsize::new(size).unwrap(),
                        "Cache capacity mismatch for config: {}",
                        config
                    );
                }
                None => {
                    assert!(
                        root_ctx.cache.is_none(),
                        "Cache should be disabled for config: {}",
                        config
                    );
                }
            }
        }
    }

    #[test]
    fn test_cache_disabled_behavior() {
        let mut root_ctx = OpenapiPathRoot::new();
        let config = json!({
            "cache_size": 0,
            "paths": {
                "/api/users/{id}": {},
            }
        })
        .to_string();
        root_ctx
            .configure(&serde_json::from_str(&config).unwrap())
            .unwrap();

        let http_ctx = OpenapiPathFilter {
            router: Rc::clone(&root_ctx.router),
            cache: root_ctx.cache.as_ref().map(Rc::clone),
        };

        let path = "/api/users/123";
        let result1 = http_ctx.get_openapi_path(path);
        assert_eq!(result1, Some("/api/users/{id}".to_string()));
        let result2 = http_ctx.get_openapi_path(path);
        assert_eq!(result2, Some("/api/users/{id}".to_string()));

        assert!(root_ctx.cache.is_none(), "Cache should be disabled");
    }

    #[test]
    fn test_invalid_config() {
        let mut context = OpenapiPathRoot::new();
        let invalid_configs = vec![
            json!({}),
            json!({"paths": "string"}),
            json!({"paths": ["array"]}),
        ];

        for config in invalid_configs {
            assert!(
                context.configure(&config).is_err(),
                "Should fail to configure with invalid config: {}",
                config
            );
        }
    }

    #[test]
    fn test_empty_paths() {
        let mut root_ctx = OpenapiPathRoot::new();
        let config = json!({
            "cache_size": 1024,
            "paths": {}
        })
        .to_string();
        root_ctx
            .configure(&serde_json::from_str(&config).unwrap())
            .unwrap();

        let http_ctx = OpenapiPathFilter {
            router: Rc::clone(&root_ctx.router),
            cache: root_ctx.cache.as_ref().map(Rc::clone),
        };

        assert_eq!(
            http_ctx.get_openapi_path("/api/users/123"),
            None,
            "No paths should match with empty configuration"
        );
    }
}
