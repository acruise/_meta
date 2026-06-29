//! End-to-end test against a real Postgres.
//!
//! Skipped unless `TEST_DATABASE_URL` is set, e.g.:
//!
//! ```sh
//! TEST_DATABASE_URL=postgres://localhost/cp_test cargo test -- --nocapture
//! ```
//!
//! It drops and recreates this crate's tables on the target database, so point
//! it only at a throwaway database.

use meta_control_plane::{ControlPlaneStore, EntityId, Ref, TypeId, VersionId};

const DROP_ALL: &str =
    "drop table if exists epoch_entity_type_selections, epoch_entity_selections, epochs, \
     entity_edges, entity_versions, entities, entity_type_edges, entity_type_versions, \
     entity_types cascade";

async fn fresh_store() -> Option<ControlPlaneStore> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let store = ControlPlaneStore::connect(&url).expect("build store");
    store.pool().get().await.unwrap().batch_execute(DROP_ALL).await.unwrap();
    store.migrate().await.unwrap();
    Some(store)
}

#[tokio::test]
async fn type_binding_and_closure() {
    let Some(store) = fresh_store().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };

    // Two types, one importing the other (type -> type edge).
    let address = TypeId::new("address");
    let order = TypeId::new("order");
    store.create_type(&address).await.unwrap();
    store.create_type(&order).await.unwrap();
    let addr_v1 = store.put_type_version(&address, b"address schema v1", "json").await.unwrap();
    let order_v1 = store.put_type_version(&order, b"order schema v1", "json").await.unwrap();
    assert_eq!(addr_v1, VersionId(1));
    assert_eq!(order_v1, VersionId(1));
    // order's schema pins address's schema.
    store.add_type_edge((&order, order_v1), &address, Ref::Pinned(addr_v1)).await.unwrap();

    // An instance whose header points to the `order` type identity.
    let o = EntityId::new("order-4711");
    store.create_entity(&o, &order).await.unwrap();
    // Its revision pins the type revision current at write time.
    let o_v1 = store.put_version(&o, b"the order", order_v1).await.unwrap();
    assert_eq!(o_v1, VersionId(1));

    // A second instance the order floats a reference to ("latest customer").
    let cust_type = TypeId::new("customer");
    store.create_type(&cust_type).await.unwrap();
    let cust_tv = store.put_type_version(&cust_type, b"customer schema v1", "json").await.unwrap();
    let c = EntityId::new("cust-1");
    store.create_entity(&c, &cust_type).await.unwrap();
    store.put_version(&c, b"customer v1", cust_tv).await.unwrap();
    store.add_edge((&o, o_v1), &c, Ref::Floating).await.unwrap();

    // Cut an epoch and materialize the order's closure.
    let epoch = store.cut_epoch().await.unwrap();
    let closure = store.read_closure(epoch, &o).await.unwrap();

    // Reachable instances: the order itself + the floated customer (resolved to v1).
    let mut entities: Vec<_> = closure.entities.iter().map(|e| e.entity_id.0.clone()).collect();
    entities.sort();
    assert_eq!(entities, vec!["cust-1".to_string(), "order-4711".to_string()]);

    // Reachable types: order (pinned by the order instance), address (imported by
    // order's schema), customer (pinned by the customer instance).
    let mut types: Vec<_> = closure.types.iter().map(|t| t.type_id.0.clone()).collect();
    types.sort();
    assert_eq!(types, vec!["address".to_string(), "customer".to_string(), "order".to_string()]);

    // The order revision is bound to the order type at the revision level.
    let order_rev = closure.entities.iter().find(|e| e.entity_id.0 == "order-4711").unwrap();
    assert_eq!(order_rev.type_id, order);
    assert_eq!(order_rev.type_version_id, order_v1);
}

#[tokio::test]
async fn revision_type_version_must_exist() {
    let Some(store) = fresh_store().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };

    let a = TypeId::new("type-a");
    store.create_type(&a).await.unwrap();
    store.put_type_version(&a, b"a v1", "json").await.unwrap(); // only v1 exists

    let e = EntityId::new("e1");
    store.create_entity(&e, &a).await.unwrap();

    // The revision selects a version *number* of its declared type (the header
    // fixes the type identity; the API cannot pin to another type's revision).
    // Selecting a type-version that does not exist is rejected by the FK from
    // (type_id, type_version_id) into type_versions.
    let err = store.put_version(&e, b"payload", VersionId(99)).await;
    assert!(err.is_err(), "expected FK violation pinning to a nonexistent type version");

    // Selecting the real type version succeeds.
    let v = store.put_version(&e, b"payload", VersionId(1)).await.unwrap();
    assert_eq!(v, VersionId(1));
}

#[tokio::test]
async fn current_type_pins_latest_revision() {
    let Some(store) = fresh_store().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };

    let t = TypeId::new("widget");
    store.create_type(&t).await.unwrap();
    store.put_type_version(&t, b"widget v1", "json").await.unwrap();
    store.put_type_version(&t, b"widget v2", "json").await.unwrap();

    let e = EntityId::new("w-1");
    store.create_entity(&e, &t).await.unwrap();

    // "Current at the time" resolves to v2 and is then materialized as a pin.
    let v = store.put_version_current_type(&e, b"payload").await.unwrap();
    let rev = store.get_version(&e, v).await.unwrap();
    assert_eq!(rev.type_version_id, VersionId(2));

    // A later type revision does not retroactively move the pin.
    store.put_type_version(&t, b"widget v3", "json").await.unwrap();
    let rev_again = store.get_version(&e, v).await.unwrap();
    assert_eq!(rev_again.type_version_id, VersionId(2));
}
