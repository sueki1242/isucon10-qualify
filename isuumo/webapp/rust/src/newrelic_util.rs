#[cfg(feature = "use_newrelic")]
#[macro_use]
pub mod detail {
    use lazy_static::lazy_static;
    use newrelic::{App, Transaction};
    use std::env;

    const APP_NAME: &str = "isucon10-qual";

    lazy_static! {
        pub static ref APP: NewRelicAppData = NewRelicAppData::new();
    }

    // For handy access to global instance outside this module.
    #[allow(unused_macros)]
    macro_rules! newrelic_app {
        () => {
            crate::newrelic_util::detail::APP
        };
    }

    #[allow(unused_macros)]
    macro_rules! newrelic_init {
        () => {
            lazy_static::initialize(&newrelic_app!());
        };
    }

    #[allow(unused_macros)]
    macro_rules! newrelic_transaction {
        ($name:expr) => {
            let _transaction = newrelic_app!().transaction($name);
        };
    }

    pub struct NewRelicAppData {
        app: Option<App>,
    }

    impl NewRelicAppData {
        pub fn new() -> NewRelicAppData {
            match env::var("NEW_RELIC_LICENSE_KEY") {
                Ok(key) => {
                    let app = App::new(APP_NAME, &key).expect("Could not create app.");
                    NewRelicAppData { app: Some(app) }
                }
                Err(e) => {
                    eprintln!("{}", e);
                    NewRelicAppData { app: None }
                }
            }
        }

        pub fn transaction(&self, name: &str) -> Option<Transaction> {
            self.app.as_ref().map(|app| {
                app.web_transaction(name)
                    .expect("Could not start transaction")
            })
        }
    }
}

#[cfg(not(feature = "use_newrelic"))]
#[macro_use]
mod detail {
    #[allow(unused_macros)]
    macro_rules! newrelic_init {
        () => {};
    }

    #[allow(unused_macros)]
    macro_rules! newrelic_transaction {
        ($($_:expr),*) => {};
    }
}
