use gridcraft_formula::{Expr, parse, print};

#[test]
fn number_near_f64_max_round_trips() {
    for src in ["=1.7976931348623157E308", "=-1.7976931348623157E308"] {
        let e = parse(src).unwrap();
        let printed = print(&e);
        let back = parse(&printed).unwrap_or_else(|err| panic!("{printed}: {err:?}"));
        assert_eq!(print(&back), printed);
        match (&e, &back) {
            (Expr::Number(a), Expr::Number(b)) => assert_eq!(a, b),
            _ => assert_eq!(e, back),
        }
    }
}
