//! Точка входа web для `dx serve` / `dx bundle`: всё в [`web::Root`].

fn main() {
    dioxus::launch(web::Root);
}
