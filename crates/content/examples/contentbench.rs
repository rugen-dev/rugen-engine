//! Import and load timings of characters and stages under a content root.
//!
//! `cargo run --release -p rugen-content --example contentbench -- <root> <char id>... [--stages <stage id>...]`
//!
//! Per entry: the first prepare (import: conversion to `.rgc` / `.rgs` and
//! the manifest), a cached prepare (what the select screen pays), and the
//! median of twenty loads (characters: `CharData::load_with_data`, what a
//! fight pays; stages: def + sprites).

use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::PathBuf::from(args.first().expect("usage: contentbench <root> ..."));
    let split = args
        .iter()
        .position(|a| a == "--stages")
        .unwrap_or(args.len());
    let chars = &args[1..split];
    let stages = if split < args.len() {
        &args[split + 1..]
    } else {
        &[][..]
    };
    let lib = rugen_content::Library::discover(Some(&root));
    let data = lib.data_dir();
    for id in chars {
        let e = lib
            .find_char(id)
            .unwrap_or_else(|| panic!("{id}: not found"));
        let t = Instant::now();
        let (dir, m) = lib.prepare_char(&e).unwrap();
        let first = ms(t);
        let t = Instant::now();
        let (dir2, _) = lib.prepare_char(&e).unwrap();
        let cached = ms(t);
        assert_eq!(dir, dir2);
        let def = m.def_path(&dir);
        let loads = (0..20)
            .map(|_| {
                let t = Instant::now();
                let d = rugen_sim::CharData::load_with_data(&def, &data).unwrap();
                std::hint::black_box(d);
                ms(t)
            })
            .collect();
        println!(
            "char {id}: import {first:.0} ms, cached prepare {cached:.1} ms, fight load median {:.1} ms, fp {}",
            median(loads),
            m.fingerprint.as_deref().unwrap_or("-")
        );
    }
    for id in stages {
        let e = lib
            .stages()
            .into_iter()
            .find(|s| s.id.eq_ignore_ascii_case(id))
            .unwrap_or_else(|| panic!("{id}: not found"));
        let t = Instant::now();
        let (dir, m) = lib.prepare_stage(&e).unwrap();
        let first = ms(t);
        let t = Instant::now();
        lib.prepare_stage(&e).unwrap();
        let cached = ms(t);
        let def = m.def_path(&dir);
        let loads = (0..20)
            .map(|_| {
                let t = Instant::now();
                let d = rugen_formats::def::Def::open(&def).unwrap();
                let spr = d
                    .section("bgdef")
                    .unwrap()
                    .raw("spr")
                    .unwrap()
                    .trim()
                    .to_string();
                let sff_path = rugen_sim::chardata::resolve(def.parent().unwrap(), &spr).unwrap();
                let sff = rugen_formats::sff::Sff::open(&sff_path, false).unwrap();
                std::hint::black_box(sff);
                ms(t)
            })
            .collect();
        println!(
            "stage {id}: import {first:.0} ms, cached prepare {cached:.1} ms, load median {:.1} ms, fp {}",
            median(loads),
            m.fingerprint.as_deref().unwrap_or("-")
        );
    }
}
