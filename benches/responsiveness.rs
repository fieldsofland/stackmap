mod fixture_builder;

use std::time::Instant;

use stackmap::model::topology::{OrderMode, ProjectionOptions, TopologyIndex};

fn main() {
    for (count, iterations) in [(500, 1_000), (5_000, 100)] {
        let snapshot = fixture_builder::snapshot(count, 10);
        let started = Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(TopologyIndex::build(&snapshot));
        }
        println!(
            "{iterations} structural indexes of {count} branches: {:?}",
            started.elapsed()
        );

        let index = TopologyIndex::build(&snapshot);
        for (label, options) in [
            ("recent", ProjectionOptions::default()),
            (
                "alphabetical",
                ProjectionOptions {
                    order: OrderMode::Alphabetical,
                    ..ProjectionOptions::default()
                },
            ),
            (
                "graphite",
                ProjectionOptions {
                    order: OrderMode::Graphite,
                    ..ProjectionOptions::default()
                },
            ),
            (
                "compact",
                ProjectionOptions {
                    separators: false,
                    ..ProjectionOptions::default()
                },
            ),
        ] {
            let started = Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(index.project(&options));
            }
            println!(
                "{iterations} {label} projections of {count} branches: {:?}",
                started.elapsed()
            );
        }
    }

    let snapshot = fixture_builder::snapshot(5_000, 5_000);
    let index = TopologyIndex::build(&snapshot);
    let options = ProjectionOptions::default();
    let iterations = 10;
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(index.project(&options));
    }
    println!(
        "{iterations} recent projections of one 5000-branch deep stack: {:?}",
        started.elapsed()
    );
}
