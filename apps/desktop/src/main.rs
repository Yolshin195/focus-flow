//! Точка входа desktop для `dx serve` / `dx build`: всё в [`desktop::launch`].

fn main() {
    desktop::prepare_env();
    desktop::launch(desktop::Options::from_env(), || {});
}
