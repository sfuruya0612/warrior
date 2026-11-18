//! IndexedDB の接続、スキーマ、トランザクション、リクエストの薄いラッパー。
//!
//! `web-sys` の API を直接使い、汎用の IndexedDB クレートは追加しない (ADR-025)。
//! スキーマは版で管理し、移行に失敗してもデータベースを削除しない (ADR-002)。

use std::cell::Cell;
use std::rc::Rc;

use js_sys::{Array, Promise, Reflect};
use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    DomStringList, IdbCursorWithValue, IdbDatabase, IdbFactory, IdbObjectStore,
    IdbObjectStoreParameters, IdbOpenDbRequest, IdbRequest, IdbTransaction, IdbTransactionMode,
};

use super::StorageError;

/// オブジェクトストアのキーの形。
enum KeyPath {
    /// 1 つの列。
    Single(&'static str),
    /// 複数の列。
    Multiple(&'static [&'static str]),
}

/// オブジェクトストアの索引。
struct IndexSpec {
    /// 索引の名前。
    name: &'static str,
    /// 索引のキーの形。
    key_path: KeyPath,
    /// 追加したスキーマの版。
    since: u32,
}

/// 必要なオブジェクトストアと索引。
struct StoreSpec {
    /// ストアの名前。
    name: &'static str,
    /// 主キーの形。
    key_path: KeyPath,
    /// 索引。
    indexes: &'static [IndexSpec],
    /// 追加したスキーマの版。
    since: u32,
}

const STORES: &[StoreSpec] = &[
    StoreSpec {
        name: "training_sessions",
        key_path: KeyPath::Single("id"),
        indexes: &[
            IndexSpec {
                name: "by_started_at",
                key_path: KeyPath::Single("started_at"),
                since: 1,
            },
            IndexSpec {
                name: "by_date",
                key_path: KeyPath::Single("date"),
                since: 2,
            },
        ],
        since: 1,
    },
    StoreSpec {
        name: "training_exercises",
        key_path: KeyPath::Single("id"),
        indexes: &[IndexSpec {
            name: "by_training",
            key_path: KeyPath::Single("training_id"),
            since: 1,
        }],
        since: 1,
    },
    StoreSpec {
        name: "sets",
        key_path: KeyPath::Single("id"),
        indexes: &[IndexSpec {
            name: "by_training_exercise",
            key_path: KeyPath::Single("training_exercise_id"),
            since: 1,
        }],
        since: 1,
    },
    StoreSpec {
        name: "set_segments",
        key_path: KeyPath::Single("id"),
        indexes: &[IndexSpec {
            name: "by_set",
            key_path: KeyPath::Single("set_id"),
            since: 1,
        }],
        since: 1,
    },
    StoreSpec {
        name: "supersets",
        key_path: KeyPath::Single("id"),
        indexes: &[IndexSpec {
            name: "by_training",
            key_path: KeyPath::Single("training_id"),
            since: 1,
        }],
        since: 1,
    },
    StoreSpec {
        name: "superset_members",
        key_path: KeyPath::Multiple(&["superset_id", "training_exercise_id"]),
        indexes: &[IndexSpec {
            name: "by_training_exercise",
            key_path: KeyPath::Single("training_exercise_id"),
            since: 1,
        }],
        since: 1,
    },
    StoreSpec {
        name: "outbox",
        key_path: KeyPath::Single("operation.operation_id"),
        indexes: &[IndexSpec {
            name: "by_training_sequence",
            key_path: KeyPath::Multiple(&["operation.training_id", "operation.sequence"]),
            since: 1,
        }],
        since: 1,
    },
    StoreSpec {
        name: "exercise_outbox",
        key_path: KeyPath::Single("operation_id"),
        indexes: &[],
        since: 4,
    },
    StoreSpec {
        name: "meta",
        key_path: KeyPath::Single("key"),
        indexes: &[],
        since: 1,
    },
    StoreSpec {
        name: "body_weight",
        key_path: KeyPath::Single("key"),
        indexes: &[],
        since: 3,
    },
    StoreSpec {
        name: "routines",
        key_path: KeyPath::Single("id"),
        indexes: &[IndexSpec {
            name: "by_position",
            key_path: KeyPath::Single("position"),
            since: 4,
        }],
        since: 4,
    },
    StoreSpec {
        name: "library_exercises",
        key_path: KeyPath::Single("id"),
        indexes: &[],
        since: 5,
    },
    StoreSpec {
        name: "recorded_exercises",
        key_path: KeyPath::Single("exercise_id"),
        indexes: &[],
        since: 5,
    },
];

/// データベースを開く。
pub async fn open(name: &str, version: u32) -> Result<IdbDatabase, StorageError> {
    let factory = factory()?;
    let request = factory
        .open_with_u32(name, version)
        .map_err(|error| StorageError::Open(describe(&error)))?;
    let settled = Rc::new(Cell::new(false));
    let promise = Promise::new(&mut |resolve, reject| {
        request.set_onupgradeneeded(Some(
            upgrade_handler(settled.clone(), reject.clone(), version).unchecked_ref(),
        ));
        request.set_onblocked(Some(
            blocked_handler(settled.clone(), reject.clone()).unchecked_ref(),
        ));
        request.set_onsuccess(Some(
            open_success_handler(settled.clone(), resolve, reject.clone()).unchecked_ref(),
        ));
        request.set_onerror(Some(
            open_error_handler(settled.clone(), reject).unchecked_ref(),
        ));
    });

    let value = await_js(promise).await.map_err(StorageError::Open)?;
    let database: IdbDatabase = value
        .dyn_into()
        .map_err(|_| StorageError::Open("the open result is not a database".to_owned()))?;
    close_on_version_change(&database);
    Ok(database)
}

fn upgrade_handler(
    settled: Rc<Cell<bool>>,
    reject: js_sys::Function,
    version: u32,
) -> js_sys::Function {
    Closure::once_into_js(move |event: web_sys::Event| {
        let Some(request) = event_target::<IdbOpenDbRequest>(&event) else {
            settled.set(true);
            let _ = reject.call1(
                &JsValue::NULL,
                &JsValue::from_str("the upgrade request is missing"),
            );
            return;
        };
        let database = request.result().and_then(|value| {
            value
                .dyn_into::<IdbDatabase>()
                .map_err(|_| JsValue::from_str("the upgrade result is not a database"))
        });
        let database = match database {
            Ok(database) => database,
            Err(error) => {
                settled.set(true);
                let _ = reject.call1(&JsValue::NULL, &error);
                return;
            }
        };
        let Some(transaction) = request.transaction() else {
            settled.set(true);
            let _ = reject.call1(
                &JsValue::NULL,
                &JsValue::from_str("the upgrade transaction is missing"),
            );
            return;
        };
        if let Err(error) = ensure_schema(&database, &transaction, version) {
            let _ = transaction.abort();
            settled.set(true);
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&error.to_string()));
        }
    })
    .unchecked_into()
}

fn blocked_handler(settled: Rc<Cell<bool>>, reject: js_sys::Function) -> js_sys::Function {
    Closure::once_into_js(move |_event: web_sys::Event| {
        settled.set(true);
        let _ = reject.call1(
            &JsValue::NULL,
            &JsValue::from_str("the database upgrade is blocked by another connection"),
        );
    })
    .unchecked_into()
}

fn open_success_handler(
    settled: Rc<Cell<bool>>,
    resolve: js_sys::Function,
    reject: js_sys::Function,
) -> js_sys::Function {
    Closure::once_into_js(move |event: web_sys::Event| {
        let Some(request) = event_target::<IdbOpenDbRequest>(&event) else {
            let _ = reject.call1(
                &JsValue::NULL,
                &JsValue::from_str("the open request is missing"),
            );
            return;
        };
        if settled.get() {
            if let Ok(database) = request.result() {
                if let Ok(database) = database.dyn_into::<IdbDatabase>() {
                    database.close();
                }
            }
            return;
        }
        match request.result() {
            Ok(database) => {
                let _ = resolve.call1(&JsValue::NULL, &database);
            }
            Err(error) => {
                let _ = reject.call1(&JsValue::NULL, &error);
            }
        }
    })
    .unchecked_into()
}

fn open_error_handler(settled: Rc<Cell<bool>>, reject: js_sys::Function) -> js_sys::Function {
    Closure::once_into_js(move |event: web_sys::Event| {
        if settled.get() {
            return;
        }
        let message = event_target::<IdbRequest>(&event)
            .and_then(|request| request.error().ok().flatten())
            .map(|error| describe(&error))
            .unwrap_or_else(|| "the database could not be opened".to_owned());
        let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&message));
    })
    .unchecked_into()
}

/// データベースを削除する。接続を閉じた後に呼ぶ。
pub async fn delete(name: &str) -> Result<(), StorageError> {
    let factory = factory()?;
    let request = factory
        .delete_database(name)
        .map_err(|error| StorageError::Open(describe(&error)))?;
    await_request(request.unchecked_into())
        .await
        .map(|_| ())
        .map_err(|error| StorageError::Open(error.to_string()))
}

/// IndexedDB が使えなければエラーにする。
fn factory() -> Result<IdbFactory, StorageError> {
    let window = web_sys::window().ok_or_else(|| {
        StorageError::Unavailable("the browser window is not available".to_owned())
    })?;
    window
        .indexed_db()
        .map_err(|error| StorageError::Unavailable(describe(&error)))?
        .ok_or_else(|| StorageError::Unavailable("IndexedDB is not available".to_owned()))
}

fn close_on_version_change(database: &IdbDatabase) {
    let database_for_event = database.clone();
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
        database_for_event.close();
    });
    database.set_onversionchange(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
}

fn ensure_schema(
    database: &IdbDatabase,
    transaction: &IdbTransaction,
    version: u32,
) -> Result<(), StorageError> {
    let names: DomStringList = database.object_store_names();
    for spec in STORES {
        if spec.since > version {
            continue;
        }
        let store = if names.contains(spec.name) {
            transaction
                .object_store(spec.name)
                .map_err(|error| StorageError::Open(describe(&error)))?
        } else {
            let parameters = IdbObjectStoreParameters::new();
            set_key_path(&parameters, &spec.key_path);
            database
                .create_object_store_with_optional_parameters(spec.name, &parameters)
                .map_err(|error| StorageError::Open(describe(&error)))?
        };
        let index_names = store.index_names();
        for index in spec.indexes {
            if index.since > version || index_names.contains(index.name) {
                continue;
            }
            match &index.key_path {
                KeyPath::Single(path) => {
                    store
                        .create_index_with_str(index.name, path)
                        .map_err(|error| StorageError::Open(describe(&error)))?;
                }
                KeyPath::Multiple(paths) => {
                    let key_path = key_path_value(paths);
                    store
                        .create_index_with_str_sequence(index.name, &key_path)
                        .map_err(|error| StorageError::Open(describe(&error)))?;
                }
            }
        }
    }
    if version >= 2 {
        let sessions = transaction
            .object_store("training_sessions")
            .map_err(|error| StorageError::Open(describe(&error)))?;
        migrate_confirmed_memo(&sessions, transaction)?;
    }
    Ok(())
}

fn migrate_confirmed_memo(
    store: &IdbObjectStore,
    transaction: &IdbTransaction,
) -> Result<(), StorageError> {
    let request = store
        .open_cursor()
        .map_err(|error| StorageError::Open(describe(&error)))?;
    let store = store.clone();
    let success_transaction = transaction.clone();
    let success = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let Some(request) = event_target::<IdbRequest>(&event) else {
            let _ = success_transaction.abort();
            return;
        };
        let Ok(result) = request.result() else {
            let _ = success_transaction.abort();
            return;
        };
        if result.is_null() || result.is_undefined() {
            return;
        }
        let Ok(cursor) = result.dyn_into::<IdbCursorWithValue>() else {
            let _ = success_transaction.abort();
            return;
        };
        let Ok(value) = cursor.value() else {
            let _ = success_transaction.abort();
            return;
        };
        let Ok(object) = value.dyn_into::<js_sys::Object>() else {
            let _ = success_transaction.abort();
            return;
        };
        let has_confirmed =
            Reflect::has(&object, &JsValue::from_str("confirmed_memo")).unwrap_or(false);
        if !has_confirmed {
            let Ok(memo) = Reflect::get(&object, &JsValue::from_str("memo")) else {
                let _ = success_transaction.abort();
                return;
            };
            if !memo.is_undefined()
                && (Reflect::set(&object, &JsValue::from_str("confirmed_memo"), &memo).is_err()
                    || store.put(&JsValue::from(object)).is_err())
            {
                let _ = success_transaction.abort();
                return;
            }
        }
        if cursor.continue_().is_err() {
            let _ = success_transaction.abort();
        }
    });
    request.set_onsuccess(Some(success.as_ref().unchecked_ref()));
    success.forget();

    let error_transaction = transaction.clone();
    let error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
        let _ = error_transaction.abort();
    });
    request.set_onerror(Some(error.as_ref().unchecked_ref()));
    error.forget();
    Ok(())
}

fn set_key_path(parameters: &IdbObjectStoreParameters, key_path: &KeyPath) {
    match key_path {
        KeyPath::Single(path) => parameters.set_key_path_opt_str(Some(path)),
        KeyPath::Multiple(paths) => {
            let value = key_path_value(paths);
            parameters.set_key_path_opt_str_sequence(Some(&value));
        }
    }
}

fn key_path_value(paths: &[&str]) -> JsValue {
    let array = Array::new();
    for path in paths {
        array.push(&JsValue::from_str(path));
    }
    array.into()
}

/// 1 つのトランザクションと、その完了を待つための Promise。
pub struct Transaction {
    inner: IdbTransaction,
    done: Promise,
}

impl Transaction {
    /// 指定したストアのトランザクションを開始する。
    pub fn new(
        database: &IdbDatabase,
        stores: &[&str],
        mode: IdbTransactionMode,
    ) -> Result<Self, StorageError> {
        let names = Array::new();
        for store in stores {
            names.push(&JsValue::from_str(store));
        }
        let inner = database
            .transaction_with_str_sequence_and_mode(&names.into(), mode)
            .map_err(|error| StorageError::Transaction(describe(&error)))?;
        let abort_transaction = inner.clone();
        let error_transaction = inner.clone();
        let done = Promise::new(&mut |resolve, reject| {
            let complete = Closure::once_into_js(move |_event: web_sys::Event| {
                let _ = resolve.call0(&JsValue::NULL);
            });
            inner.set_oncomplete(Some(complete.unchecked_ref()));

            let abort_reject = reject.clone();
            let abort_transaction = abort_transaction.clone();
            let abort = Closure::once_into_js(move |_event: web_sys::Event| {
                let error = abort_transaction
                    .error()
                    .map(|error| describe(&error))
                    .unwrap_or_else(|| "the transaction was aborted".to_owned());
                let _ = abort_reject.call1(&JsValue::NULL, &JsValue::from_str(&error));
            });
            inner.set_onabort(Some(abort.unchecked_ref()));

            let error_reject = reject;
            let error_transaction = error_transaction.clone();
            let error = Closure::once_into_js(move |_event: web_sys::Event| {
                let message = error_transaction
                    .error()
                    .map(|error| describe(&error))
                    .unwrap_or_else(|| "the transaction failed".to_owned());
                let _ = error_reject.call1(&JsValue::NULL, &JsValue::from_str(&message));
            });
            inner.set_onerror(Some(error.unchecked_ref()));
        });
        Ok(Self { inner, done })
    }

    /// トランザクションのストアを引く。
    pub fn store(&self, name: &str) -> Result<IdbObjectStore, StorageError> {
        self.inner
            .object_store(name)
            .map_err(|error| StorageError::Transaction(describe(&error)))
    }

    /// トランザクションを中断する。
    pub fn abort(&self) {
        let _ = self.inner.abort();
    }

    /// トランザクションの完了を待つ。
    pub async fn finish(self) -> Result<(), StorageError> {
        let Self { done, .. } = self;
        await_js(done)
            .await
            .map(|_| ())
            .map_err(StorageError::Transaction)
    }
}

/// リクエストの結果を待つ。
pub async fn await_request(request: IdbRequest) -> Result<JsValue, StorageError> {
    let promise = Promise::new(&mut |resolve, reject| {
        let success_reject = reject.clone();
        let success = Closure::once_into_js(move |event: web_sys::Event| {
            let Some(request) = event_target::<IdbRequest>(&event) else {
                let _ = success_reject.call1(
                    &JsValue::NULL,
                    &JsValue::from_str("the request is missing from the event"),
                );
                return;
            };
            match request.result() {
                Ok(value) => {
                    let _ = resolve.call1(&JsValue::NULL, &value);
                }
                Err(error) => {
                    let _ = success_reject.call1(&JsValue::NULL, &error);
                }
            }
        });
        request.set_onsuccess(Some(success.unchecked_ref()));

        let error_reject = reject;
        let error_request = request.clone();
        let error = Closure::once_into_js(move |_event: web_sys::Event| {
            let message = error_request
                .error()
                .ok()
                .flatten()
                .map(|error| describe(&error))
                .unwrap_or_else(|| "the request failed".to_owned());
            let _ = error_reject.call1(&JsValue::NULL, &JsValue::from_str(&message));
        });
        request.set_onerror(Some(error.unchecked_ref()));
    });
    await_js(promise).await.map_err(StorageError::Request)
}

/// ストアから 1 件を読む。
pub async fn get(store: &IdbObjectStore, key: &JsValue) -> Result<Option<JsValue>, StorageError> {
    let request = store
        .get(key)
        .map_err(|error| StorageError::Request(describe(&error)))?;
    let value = await_request(request).await?;
    Ok((!value.is_undefined() && !value.is_null()).then_some(value))
}

/// ストアのすべての行を読む。
pub async fn get_all(store: &IdbObjectStore) -> Result<Vec<JsValue>, StorageError> {
    let request = store
        .get_all()
        .map_err(|error| StorageError::Request(describe(&error)))?;
    let value = await_request(request).await?;
    let array: Array = value
        .dyn_into()
        .map_err(|_| StorageError::Request("the result is not an array".to_owned()))?;
    Ok(array.iter().collect())
}

/// ストアに 1 件を書く。
pub async fn put(store: &IdbObjectStore, value: &JsValue) -> Result<(), StorageError> {
    let request = store
        .put(value)
        .map_err(|error| StorageError::Request(describe(&error)))?;
    await_request(request).await.map(|_| ())
}

/// ストアに 1 件を追加する。同じキーがあればエラーにする。
pub async fn add(store: &IdbObjectStore, value: &JsValue) -> Result<(), StorageError> {
    let request = store
        .add(value)
        .map_err(|error| StorageError::Request(describe(&error)))?;
    await_request(request).await.map(|_| ())
}

/// ストアから 1 件を削除する。
pub async fn delete_entry(store: &IdbObjectStore, key: &JsValue) -> Result<(), StorageError> {
    let request = store
        .delete(key)
        .map_err(|error| StorageError::Request(describe(&error)))?;
    await_request(request).await.map(|_| ())
}

/// 永続ストレージの許可を求める。結果が false でも保存は続ける。
pub async fn request_persistence() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Ok(promise) = window.navigator().storage().persist() else {
        return false;
    };
    JsFuture::from(promise)
        .await
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

async fn await_js(promise: Promise) -> Result<JsValue, String> {
    JsFuture::from(promise)
        .await
        .map_err(|error| describe(&error))
}

fn event_target<T: JsCast>(event: &web_sys::Event) -> Option<T> {
    event.target()?.dyn_into().ok()
}

fn describe<T: AsRef<JsValue>>(value: &T) -> String {
    let value = value.as_ref();
    if let Some(exception) = value.dyn_ref::<web_sys::DomException>() {
        return format!("{}: {}", exception.name(), exception.message());
    }
    value
        .as_string()
        .or_else(|| {
            js_sys::JSON::stringify(value)
                .ok()
                .and_then(|value| value.as_string())
        })
        .unwrap_or_else(|| format!("{value:?}"))
}

/// 構造体を IndexedDB に書ける JavaScript の値にする。
pub fn to_js<T: Serialize>(value: &T) -> Result<JsValue, StorageError> {
    let text =
        serde_json::to_string(value).map_err(|error| StorageError::Serialize(error.to_string()))?;
    js_sys::JSON::parse(&text).map_err(|error| StorageError::Serialize(describe(&error)))
}

/// IndexedDB から読んだ JavaScript の値を構造体にする。
pub fn from_js<T: DeserializeOwned>(value: &JsValue) -> Result<T, StorageError> {
    let text = js_sys::JSON::stringify(value)
        .map_err(|error| StorageError::Serialize(describe(&error)))?
        .as_string()
        .ok_or_else(|| StorageError::Serialize("the value is not JSON".to_owned()))?;
    serde_json::from_str(&text).map_err(|error| StorageError::Serialize(error.to_string()))
}
