#![allow(missing_docs)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use pc_select::{Constraints, Method, Task, select};
use std::{collections::BTreeMap, time::Duration};

fn instance(n: usize) -> Vec<Task> {
    let mut tasks = Vec::with_capacity(n);
    tasks.push(Task {
        id: "root".into(),
        methods: vec![Method {
            id: "all-work".into(),
            costs: BTreeMap::new(),
            children: (0..n - 2).map(|i| format!("t{i}")).collect(),
            order_edges: vec![],
            resources: vec![],
        }],
    });
    for i in 0..n - 2 {
        tasks.push(Task {
            id: format!("t{i}"),
            methods: vec![
                Method {
                    id: format!("individual-{i}"),
                    costs: BTreeMap::from([("cost".into(), 5.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
                Method {
                    id: format!("reuse-{i}"),
                    costs: BTreeMap::from([("cost".into(), 0.5)]),
                    children: vec!["shared".into()],
                    order_edges: vec![],
                    resources: vec![],
                },
            ],
        });
    }
    tasks.push(Task {
        id: "shared".into(),
        methods: vec![Method {
            id: "shared-work".into(),
            costs: BTreeMap::from([("cost".into(), 100.0)]),
            children: vec![],
            order_edges: vec![],
            resources: vec![],
        }],
    });
    tasks
}

fn selection_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("p6_select");
    group.sample_size(10);
    for n in [100, 400, 1000] {
        let tasks = instance(n);
        let constraints = Constraints {
            weights: BTreeMap::from([("cost".into(), 1.0)]),
            ..Constraints::default()
        };
        group.bench_with_input(BenchmarkId::from_parameter(n), &tasks, |b, tasks| {
            b.iter(|| select(tasks, "root", &constraints, Duration::from_secs(10)).unwrap());
        });
    }
    group.finish();
}

criterion_group!(benches, selection_bench);
criterion_main!(benches);
