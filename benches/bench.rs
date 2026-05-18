//! Performance benchmarks for orix core operations.
//!
//! Run with: `cargo bench`
//!
//! For CI comparison, use: `cargo bench -- --test`

#![allow(clippy::unwrap_used, clippy::missing_docs_in_private_items)]

use std::collections::BTreeMap;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use orix_domain::{PackageId, PackageName, ResolvedPackage, Version};

// ── Domain type benchmarks ──────────────────────────────────────────────────

fn domain_package_name_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("domain::PackageName::from");

    group.bench_function("parse_scoped", |b| {
        b.iter(|| PackageName::from(black_box("@babel/core@7.26.0")))
    });

    group.bench_function("parse_unscoped", |b| {
        b.iter(|| PackageName::from(black_box("lodash@4.17.21")))
    });

    group.bench_function("parse_normalize", |b| {
        b.iter(|| PackageName::from(black_box("@scope/PKG@1.0.0")))
    });

    group.finish();
}

fn domain_version_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("domain::Version::parse");

    group.bench_function("parse_standard", |b| {
        b.iter(|| Version::parse(black_box("1.2.3")))
    });

    group.bench_function("parse_prerelease", |b| {
        b.iter(|| Version::parse(black_box("2.0.0-alpha.1")))
    });

    group.bench_function("parse_build_metadata", |b| {
        b.iter(|| Version::parse(black_box("3.1.0+build.123")))
    });

    group.bench_function("parse_many", |b| {
        let versions = ["1.0.0", "1.1.0", "2.0.0", "2.1.0", "3.0.0-beta.1"];
        b.iter(|| {
            for v in &versions {
                let _ = Version::parse(black_box(*v));
            }
        })
    });

    group.finish();
}

fn domain_package_id_key(c: &mut Criterion) {
    let mut group = c.benchmark_group("domain::PackageId::key");

    let id = PackageId::new(
        PackageName::from("@babel/core"),
        Version::parse("7.26.0").unwrap(),
    );
    let id_unscoped = PackageId::new(
        PackageName::from("lodash"),
        Version::parse("4.17.21").unwrap(),
    );

    group.bench_function("scoped_package", |b| b.iter(|| black_box(&id).key()));

    group.bench_function("unscoped_package", |b| {
        b.iter(|| black_box(&id_unscoped).key())
    });

    group.finish();
}

// ── Manifest benchmarks ────────────────────────────────────────────────────────

fn manifest_parse_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("manifest::parse");

    let small_pkg = r#"{"name":"test","version":"1.0.0","dependencies":{"lodash":"^4.17.21"}}"#;
    group.bench_function("minimal_package_json", |b| {
        b.iter(|| orix_manifest::Manifest::parse_str(black_box(small_pkg), "benchmark"))
    });

    let with_scripts = r#"{
  "name":"app",
  "version":"2.0.0",
  "scripts":{"build":"tsc","test":"jest"},
  "dependencies":{"react":"^18.0.0","lodash":"^4.17.21"},
  "devDependencies":{"typescript":"^5.0.0","@types/react":"^18.0.0"}
}"#;
    group.bench_function("typical_package_json", |b| {
        b.iter(|| orix_manifest::Manifest::parse_str(black_box(with_scripts), "benchmark"))
    });

    let large_pkg = r#"{
  "name":"heavy-lib",
  "version":"10.0.0",
  "dependencies":{
    "a":"1.0.0","b":"1.0.0","c":"1.0.0","d":"1.0.0","e":"1.0.0",
    "f":"1.0.0","g":"1.0.0","h":"1.0.0","i":"1.0.0","j":"1.0.0",
    "k":"1.0.0","l":"1.0.0","m":"1.0.0","n":"1.0.0","o":"1.0.0",
    "p":"1.0.0","q":"1.0.0","r":"1.0.0","s":"1.0.0","t":"1.0.0"
  }
}"#;
    group.bench_function("large_dep_list_20_deps", |b| {
        b.iter(|| orix_manifest::Manifest::parse_str(black_box(large_pkg), "benchmark"))
    });

    group.finish();
}

// ── Lockfile benchmarks ───────────────────────────────────────────────────────

fn lockfile_empty(c: &mut Criterion) {
    let mut group = c.benchmark_group("lockfile::empty");

    group.bench_function("create_empty_lockfile", |b| {
        b.iter(orix_lockfile::Lockfile::empty)
    });

    group.finish();
}

fn lockfile_update_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("lockfile::update");

    let manifest = orix_manifest::Manifest {
        name: Some("app".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: [
            ("react".to_string(), "^18.0.0".to_string()),
            ("lodash".to_string(), "^4.17.21".to_string()),
        ]
        .into_iter()
        .collect(),
        dev_dependencies: [("typescript".to_string(), "^5.0.0".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    };

    let mut graph = orix_domain::DependencyGraph::new();
    let react_id = PackageId::new(
        PackageName::from("react"),
        Version::parse("18.2.0").unwrap(),
    );
    let lodash_id = PackageId::new(
        PackageName::from("lodash"),
        Version::parse("4.17.21").unwrap(),
    );
    let ts_id = PackageId::new(
        PackageName::from("typescript"),
        Version::parse("5.0.0").unwrap(),
    );

    graph.insert(ResolvedPackage {
        id: react_id,
        integrity: "sha512-abc".to_string(),
        tarball: "https://registry.npmjs.org/react/-/react-18.2.0.tgz".to_string(),
        dependencies: Vec::new(),
        dev_dependencies: Vec::new(),
        optional_dependencies: Vec::new(),
        peer_dependencies: Vec::new(),
        engines: None,
        os: Vec::new(),
        cpu: Vec::new(),
        depnodes: Vec::new(),
        patch: None,
    });
    graph.insert(ResolvedPackage {
        id: lodash_id.clone(),
        integrity: "sha512-def".to_string(),
        tarball: "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz".to_string(),
        dependencies: Vec::new(),
        dev_dependencies: Vec::new(),
        optional_dependencies: Vec::new(),
        peer_dependencies: Vec::new(),
        engines: None,
        os: Vec::new(),
        cpu: Vec::new(),
        depnodes: Vec::new(),
        patch: None,
    });
    graph.insert(ResolvedPackage {
        id: ts_id,
        integrity: "sha512-ghi".to_string(),
        tarball: "https://registry.npmjs.org/typescript/-/typescript-5.0.0.tgz".to_string(),
        dependencies: Vec::new(),
        dev_dependencies: Vec::new(),
        optional_dependencies: Vec::new(),
        peer_dependencies: Vec::new(),
        engines: None,
        os: Vec::new(),
        cpu: Vec::new(),
        depnodes: Vec::new(),
        patch: None,
    });

    group.bench_function("update_with_3_packages", |b| {
        b.iter(|| {
            orix_lockfile::Lockfile::empty().update(black_box(&manifest), black_box(&graph), ".")
        })
    });

    group.finish();
}

fn lockfile_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("lockfile::serialize");

    let mut lockfile = orix_lockfile::Lockfile::empty();
    for i in 0..50 {
        let name = format!("pkg-{}", i);
        lockfile.packages.insert(
            format!("/{}/{}@1.0.{}", name, name, i % 10),
            orix_lockfile::PackageLock {
                id: None,
                local: None,
                integrity: Some(format!("sha512-{}", name)),
                name: Some(name.clone()),
                version: Some(format!("1.0.{}", i % 10)),
                resolution: Some(orix_lockfile::PackageResolution {
                    tarball: Some(format!("https://registry.npmjs.org/{}/1.0.0.tgz", name)),
                    integrity: Some(format!("sha512-{}", name)),
                    resolution_type: None,
                    path: None,
                }),
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                engines: None,
                os: None,
                cpu: None,
            },
        );
    }

    group.bench_function("serialize_50_packages_yaml", |b| {
        b.iter(|| serde_yaml::to_string(black_box(&lockfile)).unwrap())
    });

    group.finish();
}

// ── Workspace spec parsing benchmarks ───────────────────────────────────────────

fn workspace_spec_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("workspace::WorkspaceSpec::parse");

    group.bench_function("workspace_star", |b| {
        b.iter(|| orix_workspace::WorkspaceSpec::parse(black_box("workspace:*")))
    });

    group.bench_function("workspace_caret", |b| {
        b.iter(|| orix_workspace::WorkspaceSpec::parse(black_box("workspace:^1.0.0")))
    });

    group.bench_function("workspace_tilde", |b| {
        b.iter(|| orix_workspace::WorkspaceSpec::parse(black_box("workspace:~2.0.0")))
    });

    group.bench_function("workspace_file", |b| {
        b.iter(|| orix_workspace::WorkspaceSpec::parse(black_box("workspace:file:../utils")))
    });

    group.bench_function("workspace_scoped", |b| {
        b.iter(|| orix_workspace::WorkspaceSpec::parse(black_box("workspace:@scope/pkg")))
    });

    group.bench_function("plain_name", |b| {
        b.iter(|| orix_workspace::WorkspaceSpec::parse(black_box("react@^18.0.0")))
    });

    group.finish();
}

// ── Utils benchmarks ─────────────────────────────────────────────────────────

fn utils_name_normalize(c: &mut Criterion) {
    let mut group = c.benchmark_group("utils::normalize_name");

    group.bench_function("scoped_name", |b| {
        b.iter(|| orix_utils::normalize_name(black_box("@babel/core")))
    });

    group.bench_function("unscoped_name", |b| {
        b.iter(|| orix_utils::normalize_name(black_box("lodash")))
    });

    group.finish();
}

// ── Dependency graph benchmarks ──────────────────────────────────────────────

fn dep_graph_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("domain::DependencyGraph::insert");

    group.bench_function("insert_sequential_100", |b| {
        b.iter(|| {
            let mut g = orix_domain::DependencyGraph::new();
            for i in 0..100 {
                let id = PackageId::new(
                    PackageName::from(format!("pkg-{}", i)),
                    Version::parse("1.0.0").unwrap(),
                );
                g.insert(ResolvedPackage {
                    id,
                    integrity: String::new(),
                    tarball: String::new(),
                    dependencies: Vec::new(),
                    dev_dependencies: Vec::new(),
                    optional_dependencies: Vec::new(),
                    peer_dependencies: Vec::new(),
                    engines: None,
                    os: Vec::new(),
                    cpu: Vec::new(),
                    depnodes: Vec::new(),
                    patch: None,
                });
            }
            // Use the graph to avoid optimizer removing the loop.
            let _ = black_box(g.packages().count());
        })
    });

    group.finish();
}

fn dep_graph_lookup(c: &mut Criterion) {
    let mut graph = orix_domain::DependencyGraph::new();
    for i in 0..100 {
        let id = PackageId::new(
            PackageName::from(format!("pkg-{}", i)),
            Version::parse("1.0.0").unwrap(),
        );
        graph.insert(ResolvedPackage {
            id,
            integrity: String::new(),
            tarball: String::new(),
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            optional_dependencies: Vec::new(),
            peer_dependencies: Vec::new(),
            engines: None,
            os: Vec::new(),
            cpu: Vec::new(),
            depnodes: Vec::new(),
            patch: None,
        });
    }

    let mut group = c.benchmark_group("domain::DependencyGraph::get");

    group.bench_function("get_existing_100_packages", |b| {
        b.iter(|| {
            for i in 0..100 {
                let name = PackageName::from(format!("pkg-{}", i));
                let version = Version::parse("1.0.0").unwrap();
                let id = PackageId::new(name, version);
                let _ = black_box(&graph).get(&id);
            }
        })
    });

    group.finish();
}

// ── Store benchmarks ──────────────────────────────────────────────────────────

fn store_import_package(c: &mut Criterion) {
    let mut group = c.benchmark_group("store::import_package");

    let temp = tempfile::tempdir().unwrap();
    let store = orix_store::Store::open(temp.path().join("store")).unwrap();
    let pkg_dir = temp.path().join("pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("package.json"),
        r#"{"name":"fixture","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(pkg_dir.join("index.js"), "module.exports = 1;\n").unwrap();

    let id = PackageId::new(PackageName::from("a"), Version::parse("1.0.0").unwrap());

    group.bench_function("first_import_copies_files", |b| {
        b.iter(|| {
            let td = tempfile::tempdir().unwrap();
            let pd = td.path().join("pkg");
            std::fs::create_dir_all(&pd).unwrap();
            std::fs::write(pd.join("package.json"), r#"{"name":"x","version":"1.0.0"}"#).unwrap();
            std::fs::write(pd.join("index.js"), "module.exports = 1;\n").unwrap();
            let pkg_id = PackageId::new(PackageName::from("x"), Version::parse("1.0.0").unwrap());
            store
                .import_package(&pkg_id, &pd, Vec::new(), None)
                .unwrap()
        })
    });

    group.bench_function("repeat_import_skipped_via_cache", |b| {
        b.iter(|| {
            store
                .import_package(&id, &pkg_dir, Vec::new(), None)
                .unwrap()
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    domain_package_name_parse,
    domain_version_parse,
    domain_package_id_key,
    manifest_parse_small,
    lockfile_empty,
    lockfile_update_small,
    lockfile_serialize,
    workspace_spec_parse,
    utils_name_normalize,
    dep_graph_insert,
    dep_graph_lookup,
    store_import_package,
);
criterion_main!(benches);
