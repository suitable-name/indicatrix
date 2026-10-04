//! Temporary scratch probe (deleted before handback).
use indicatrix::{
    color::body_color::{Illuminant, body_colors, delta_e_2000},
    optics::chromophore::{ChromophoreCatalogue, colorRecipe, resolve},
};

fn main() {
    let cat = ChromophoreCatalogue::global();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // usage: host path_mm id=amount ...
    let host = &args[0];
    let path: f64 = args[1].parse().unwrap();
    let mut r = colorRecipe::new(host, cat.data_version);
    for a in &args[2..] {
        let (k, v) = a.split_once('=').unwrap();
        if k == "treat" {
            r.treatments.push(v.to_string());
        } else {
            assert!(r.set_amount(k, v.parse().unwrap()));
        }
    }
    let (t, w) = resolve(&r, cat).unwrap();
    println!("warnings {w:?}");
    for ill in [Illuminant::D65, Illuminant::Planckian(2856.0)] {
        let c = body_colors(&t, path, ill);
        println!(
            "{ill:?}: o {:.1?} e {:.1?} unpol {:.1?} beta {:?}",
            c.o_ray.lab,
            c.e_ray.lab,
            c.unpolarised.lab,
            c.beta_ray.map(|b| b.lab)
        );
    }
    let d = body_colors(&t, path, Illuminant::D65).unpolarised.lab;
    let a = body_colors(&t, path, Illuminant::Planckian(2856.0))
        .unpolarised
        .lab;
    println!("cc dE00 unpol {:.2}", delta_e_2000(d, a));
}
