//! Behavioural tests for the engine through its public API.

use crate::*;

fn k(a1: &str) -> Key {
    CellRef::parse(a1).unwrap().key()
}

fn sheet(cells: &[(&str, &str)]) -> Sheet {
    let mut s = Sheet::new();
    s.batch(|s| {
        for (a1, raw) in cells {
            s.set_input(k(a1), raw);
        }
    });
    s
}

fn v(s: &Sheet, a1: &str) -> Value {
    s.value(k(a1))
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

fn t(x: &str) -> Value {
    Value::Text(x.into())
}

fn e(x: ErrorKind) -> Value {
    Value::Error(x)
}

/// The number in a cell (panics if it holds anything else).
fn v_num(s: &Sheet, a1: &str) -> f64 {
    match v(s, a1) {
        Value::Number(x) => x,
        other => panic!("{a1}: expected a number, got {other:?}"),
    }
}

/// A number result within floating-point noise of `expect`.
fn close(v: Value, expect: f64) {
    match v {
        Value::Number(x) => assert!((x - expect).abs() < 1e-12, "{x} vs {expect}"),
        other => panic!("expected a number, got {other:?}"),
    }
}

/// Evaluate one formula in a scratch sheet.
fn calc(formula: &str) -> Value {
    sheet(&[("Z99", formula)]).value(k("Z99"))
}

#[test]
fn arithmetic_precedence_and_coercion() {
    assert_eq!(calc("=1+2*3"), n(7.0));
    assert_eq!(calc("=(1+2)*3"), n(9.0));
    assert_eq!(calc("=-2^2"), n(4.0));
    assert_eq!(calc("=2^-1"), n(0.5));
    assert_eq!(calc("=2^3^2"), n(64.0));
    assert_eq!(calc("=10%"), n(0.1));
    assert_eq!(calc("=50%*200"), n(100.0));
    assert_eq!(calc("=10/4"), n(2.5));
    assert_eq!(calc("=\"3\"+4"), n(7.0), "numeric text coerces");
    assert_eq!(calc("=TRUE+TRUE"), n(2.0));
    assert_eq!(calc("=\"abc\"+1"), e(ErrorKind::Value));
    assert_eq!(calc("=1/0"), e(ErrorKind::DivZero));
    assert_eq!(calc("=0^0"), e(ErrorKind::Num));
    assert_eq!(calc("=0^-1"), e(ErrorKind::DivZero));
    assert_eq!(calc("=10^400"), e(ErrorKind::Num), "overflow is #NUM!");
    assert_eq!(calc("=(-8)^(1/3)"), e(ErrorKind::Num));
    assert_eq!(calc("=0.1+0.2"), n(0.1 + 0.2));
}

#[test]
fn text_and_comparisons() {
    assert_eq!(calc("=\"a\"&1&TRUE&\"b\""), t("a1TRUEb"));
    assert_eq!(calc("=1.5&\"\""), t("1.5"));
    assert_eq!(
        calc("=\"a\"=\"A\""),
        Value::Bool(true),
        "text compares without case"
    );
    assert_eq!(calc("=\"a\"<\"B\""), Value::Bool(true));
    assert_eq!(
        calc("=1<\"a\""),
        Value::Bool(true),
        "numbers sort before text"
    );
    assert_eq!(
        calc("=\"z\"<TRUE"),
        Value::Bool(true),
        "text sorts before logicals"
    );
    assert_eq!(calc("=2>=2"), Value::Bool(true));
    assert_eq!(calc("=2<>2"), Value::Bool(false));
    assert_eq!(
        calc("=\"1\"=1"),
        Value::Bool(false),
        "text 1 is not number 1"
    );
    // Blanks compare as 0, "" or FALSE.
    let s = sheet(&[
        ("B1", "=A1=0"),
        ("B2", "=A1=\"\""),
        ("B3", "=A1=FALSE"),
        ("B4", "=A1&\"x\""),
        ("B5", "=A1+5"),
        ("B6", "=A1"),
    ]);
    assert_eq!(
        vec![v(&s, "B1"), v(&s, "B2"), v(&s, "B3")],
        vec![Value::Bool(true); 3]
    );
    assert_eq!(
        (v(&s, "B4"), v(&s, "B5"), v(&s, "B6")),
        (t("x"), n(5.0), n(0.0))
    );
}

#[test]
fn errors_propagate_and_can_be_caught() {
    let s = sheet(&[
        ("A1", "=1/0"),
        ("A2", "5"),
        ("B1", "=A1+1"),
        ("B2", "=SUM(A1:A2)"),
        ("B3", "=IFERROR(A1,\"oops\")"),
        ("B4", "=IFERROR(A2,\"oops\")"),
        ("B5", "=A1&\"x\""),
        ("B6", "=ISERROR(A1)"),
        ("B7", "=IF(A1>0,1,2)"),
        ("B8", "=ABS(A1)"),
        ("B9", "=#N/A"),
        ("C1", "=IFNA(B9,0)"),
        ("C2", "=IFNA(A1,0)"),
    ]);
    assert_eq!(v(&s, "B1"), e(ErrorKind::DivZero));
    assert_eq!(v(&s, "B2"), e(ErrorKind::DivZero));
    assert_eq!(v(&s, "B3"), t("oops"));
    assert_eq!(v(&s, "B4"), n(5.0));
    assert_eq!(v(&s, "B5"), e(ErrorKind::DivZero));
    assert_eq!(v(&s, "B6"), Value::Bool(true));
    assert_eq!(v(&s, "B7"), e(ErrorKind::DivZero));
    assert_eq!(v(&s, "B8"), e(ErrorKind::DivZero));
    assert_eq!(v(&s, "C1"), n(0.0));
    assert_eq!(v(&s, "C2"), e(ErrorKind::DivZero), "IFNA only catches #N/A");
    assert_eq!(calc("=NOSUCH(1)"), e(ErrorKind::Name));
    assert_eq!(calc("=nosuchname+1"), e(ErrorKind::Name));
    assert_eq!(calc("=SUM(1,2"), e(ErrorKind::Parse));
    assert_eq!(calc("=SQRT(-1)"), e(ErrorKind::Num));
    assert_eq!(calc("=SUM()"), n(0.0));
    assert_eq!(calc("=ABS()"), e(ErrorKind::Value), "wrong argument count");
    assert_eq!(calc("=ABS(1,2)"), e(ErrorKind::Value));
}

#[test]
fn aggregates_over_ranges_and_values() {
    let s = sheet(&[
        ("A1", "1"),
        ("A2", "2"),
        ("A3", "text"),
        ("A4", "TRUE"),
        ("A5", ""),
        ("A6", "4"),
        ("B1", "=SUM(A1:A6)"),
        ("B2", "=AVERAGE(A1:A6)"),
        ("B3", "=MIN(A1:A6)"),
        ("B4", "=MAX(A1:A6)"),
        ("B5", "=COUNT(A1:A6)"),
        ("B6", "=COUNTA(A1:A6)"),
        ("B7", "=COUNTBLANK(A1:A6)"),
        ("B8", "=SUM(A1,A2,10)"),
        ("B9", "=SUM(\"3\",TRUE)"),
        ("B10", "=PRODUCT(A1:A2,A6)"),
        ("B11", "=MEDIAN(A1,A2,A6,100)"),
        ("B12", "=SUM(A:A)"),
        ("B14", "=AVERAGE(A3:A4)"),
        ("B15", "=MIN(A3:A4)"),
        ("B16", "=COUNT(1,\"x\",TRUE,A3)"),
        ("B17", "=SUM(A3)"),
        ("B18", "=SUM(\"x\")"),
    ]);
    assert_eq!(
        v(&s, "B1"),
        n(7.0),
        "text and logicals in ranges are ignored"
    );
    assert_eq!(v(&s, "B2"), n(7.0 / 3.0));
    assert_eq!((v(&s, "B3"), v(&s, "B4")), (n(1.0), n(4.0)));
    assert_eq!(v(&s, "B5"), n(3.0));
    assert_eq!(v(&s, "B6"), n(5.0));
    assert_eq!(v(&s, "B7"), n(1.0));
    assert_eq!(v(&s, "B8"), n(13.0));
    assert_eq!(v(&s, "B9"), n(4.0), "typed values coerce");
    assert_eq!(v(&s, "B10"), n(8.0));
    assert_eq!(v(&s, "B11"), n(3.0), "median of 1,2,4,100");
    assert_eq!(v(&s, "B12"), n(7.0), "whole column");
    assert_eq!(v(&s, "B14"), e(ErrorKind::DivZero), "no numbers to average");
    assert_eq!(v(&s, "B15"), n(0.0));
    assert_eq!(
        v(&s, "B16"),
        n(2.0),
        "1 and TRUE count; \"x\" and a text reference do not"
    );
    assert_eq!(v(&s, "B17"), n(0.0), "a referenced text cell is ignored");
    assert_eq!(
        v(&s, "B18"),
        e(ErrorKind::Value),
        "typed text that isn't a number is an error"
    );
}

#[test]
fn math_functions() {
    assert_eq!(calc("=ROUND(2.5,0)"), n(3.0));
    assert_eq!(calc("=ROUND(-2.5,0)"), n(-3.0));
    assert_eq!(calc("=ROUND(1.005,2)"), n(1.01));
    assert_eq!(calc("=ROUND(1234.5678,-2)"), n(1200.0));
    assert_eq!(calc("=ROUND(7.123456,3)"), n(7.123));
    assert_eq!(calc("=ROUNDUP(1.15,1)"), n(1.2));
    assert_eq!(calc("=ROUNDUP(-1.11,1)"), n(-1.2));
    assert_eq!(calc("=ROUNDDOWN(1.19,1)"), n(1.1));
    assert_eq!(calc("=ROUNDDOWN(-1.19,1)"), n(-1.1));
    assert_eq!(calc("=INT(-1.5)"), n(-2.0));
    assert_eq!(calc("=MOD(-7,3)"), n(2.0), "sign follows the divisor");
    assert_eq!(calc("=MOD(7,-3)"), n(-2.0));
    assert_eq!(calc("=MOD(5,0)"), e(ErrorKind::DivZero));
    assert_eq!(calc("=POWER(2,10)"), n(1024.0));
    assert_eq!(calc("=SQRT(16)"), n(4.0));
    assert_eq!(calc("=ABS(-3)"), n(3.0));
    assert_eq!(calc("=SIGN(-9)"), n(-1.0));
    assert_eq!(calc("=EXP(0)"), n(1.0));
    assert_eq!(calc("=LN(1)"), n(0.0));
    assert_eq!(calc("=LOG10(1000)"), n(3.0));
    close(calc("=LOG(8,2)"), 3.0);
    close(calc("=LOG(100)"), 2.0);
    assert_eq!(calc("=LN(0)"), e(ErrorKind::Num));
    assert_eq!(calc("=PI()"), n(std::f64::consts::PI));
    assert_eq!(calc("=CEILING(2.1,0.5)"), n(2.5));
    assert_eq!(calc("=FLOOR(2.9,0.5)"), n(2.5));
    assert_eq!(calc("=CEILING(5,0)"), n(0.0));
}

#[test]
fn logic_functions() {
    assert_eq!(calc("=IF(1>2,\"a\",\"b\")"), t("b"));
    assert_eq!(calc("=IF(TRUE,1)"), n(1.0));
    assert_eq!(calc("=IF(FALSE,1)"), Value::Bool(false));
    assert_eq!(calc("=IF(\"x\",1,2)"), e(ErrorKind::Value));
    assert_eq!(calc("=IF(0,1,2)"), n(2.0));
    assert_eq!(
        calc("=IF(TRUE,1,1/0)"),
        n(1.0),
        "only the chosen branch is evaluated"
    );
    assert_eq!(calc("=IF(FALSE,1/0,2)"), n(2.0));
    assert_eq!(calc("=AND(TRUE,1,2>1)"), Value::Bool(true));
    assert_eq!(calc("=AND(TRUE,FALSE)"), Value::Bool(false));
    assert_eq!(calc("=OR(FALSE,0,0)"), Value::Bool(false));
    assert_eq!(calc("=OR(FALSE,2)"), Value::Bool(true));
    assert_eq!(calc("=NOT(TRUE)"), Value::Bool(false));
    assert_eq!(calc("=AND()"), e(ErrorKind::Value));
    let s = sheet(&[
        ("A1", "TRUE"),
        ("A2", "0"),
        ("A3", "x"),
        ("B1", "=AND(A1:A3)"),
        ("B2", "=OR(A1:A3)"),
        ("B3", "=AND(A3)"),
    ]);
    assert_eq!(
        v(&s, "B1"),
        Value::Bool(false),
        "text in a range is ignored"
    );
    assert_eq!(v(&s, "B2"), Value::Bool(true));
    assert_eq!(v(&s, "B3"), e(ErrorKind::Value), "no logical values at all");
    let s = sheet(&[
        ("A1", "5"),
        ("A2", "x"),
        ("B1", "=ISNUMBER(A1)"),
        ("B2", "=ISTEXT(A2)"),
        ("B3", "=ISBLANK(A3)"),
        ("B4", "=ISBLANK(A1)"),
        ("B5", "=ISNA(NA())"),
        ("B6", "=ISERROR(1/0)"),
        ("B7", "=ISLOGICAL(TRUE)"),
        ("B8", "=ISNUMBER(A2)"),
    ]);
    let bs: Vec<Value> = (1..=8).map(|i| v(&s, &format!("B{i}"))).collect();
    assert_eq!(
        bs,
        vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false)
        ]
    );
}

#[test]
fn text_functions() {
    assert_eq!(calc("=LEN(\"héllo\")"), n(5.0));
    assert_eq!(calc("=LEN(12345)"), n(5.0));
    assert_eq!(calc("=UPPER(\"aé\")&LOWER(\"BÉ\")"), t("AÉbé"));
    assert_eq!(calc("=TRIM(\"  a   b  \")"), t("a b"));
    assert_eq!(calc("=LEFT(\"hello\",2)"), t("he"));
    assert_eq!(calc("=LEFT(\"hello\")"), t("h"));
    assert_eq!(calc("=RIGHT(\"hello\",3)"), t("llo"));
    assert_eq!(calc("=RIGHT(\"hi\",10)"), t("hi"));
    assert_eq!(calc("=LEFT(\"hi\",-1)"), e(ErrorKind::Value));
    assert_eq!(calc("=MID(\"abcdef\",2,3)"), t("bcd"));
    assert_eq!(calc("=MID(\"abc\",5,2)"), t(""));
    assert_eq!(calc("=MID(\"abc\",0,2)"), e(ErrorKind::Value));
    assert_eq!(calc("=CONCATENATE(\"a\",1,TRUE)"), t("a1TRUE"));
    assert_eq!(calc("=SUBSTITUTE(\"a-b-c\",\"-\",\"+\")"), t("a+b+c"));
    assert_eq!(calc("=SUBSTITUTE(\"a-b-c\",\"-\",\"+\",2)"), t("a-b+c"));
    assert_eq!(calc("=SUBSTITUTE(\"abc\",\"\",\"x\")"), t("abc"));
    assert_eq!(calc("=FIND(\"b\",\"abcb\")"), n(2.0));
    assert_eq!(calc("=FIND(\"b\",\"abcb\",3)"), n(4.0));
    assert_eq!(
        calc("=FIND(\"B\",\"abc\")"),
        e(ErrorKind::Value),
        "FIND is case-sensitive"
    );
    assert_eq!(calc("=SEARCH(\"B\",\"abc\")"), n(2.0));
    assert_eq!(
        calc("=FIND(\"é\",\"café\")"),
        n(4.0),
        "positions count characters"
    );
    assert_eq!(calc("=REPT(\"ab\",3)"), t("ababab"));
    assert_eq!(calc("=REPT(\"x\",-1)"), e(ErrorKind::Value));
    assert_eq!(calc("=EXACT(\"a\",\"A\")"), Value::Bool(false));
    assert_eq!(calc("=VALUE(\"1,234.5\")"), n(1234.5));
    assert_eq!(calc("=VALUE(\"abc\")"), e(ErrorKind::Value));
    let s = sheet(&[
        ("A1", "x"),
        ("A2", ""),
        ("A3", "y"),
        ("B1", "=TEXTJOIN(\"-\",TRUE,A1:A3)"),
        ("B2", "=TEXTJOIN(\"-\",FALSE,A1,A2,A3)"),
        ("B3", "=CONCAT(A1:A3)"),
    ]);
    assert_eq!(v(&s, "B1"), t("x-y"));
    assert_eq!(v(&s, "B2"), t("x--y"));
    assert_eq!(v(&s, "B3"), t("xy"));
}

#[test]
fn conditional_aggregates_and_criteria() {
    let s = sheet(&[
        ("A1", "apple"),
        ("A2", "Banana"),
        ("A3", "apricot"),
        ("A4", "cherry"),
        ("A5", "apple"),
        ("B1", "10"),
        ("B2", "20"),
        ("B3", "30"),
        ("B4", "40"),
        ("B5", "50"),
        ("C1", "=COUNTIF(A1:A5,\"apple\")"),
        ("C2", "=COUNTIF(A1:A5,\"AP*\")"),
        ("C3", "=COUNTIF(A1:A5,\"?herry\")"),
        ("C4", "=SUMIF(A1:A5,\"apple\",B1:B5)"),
        ("C5", "=SUMIF(B1:B5,\">25\")"),
        ("C6", "=COUNTIF(B1:B5,\"<=20\")"),
        ("C7", "=SUMIF(B1:B5,\"<>30\")"),
        ("C8", "=COUNTIF(B1:B5,30)"),
        ("C9", "=AVERAGEIF(A1:A5,\"apple\",B1:B5)"),
        ("C10", "=AVERAGEIF(A1:A5,\"zzz\",B1:B5)"),
        ("C11", "=COUNTIF(A1:A5,\"<>apple\")"),
        ("C12", "=SUMIF(A1:A5,\"a*\",B1:B5)"),
        ("C13", "=COUNTIF(B1:B5,\">=\"&B3)"),
        ("C14", "=COUNTIF(A1:A5,\"=Banana\")"),
    ]);
    let got: Vec<Value> = (1..=14).map(|i| v(&s, &format!("C{i}"))).collect();
    assert_eq!(
        got,
        vec![
            n(2.0),
            n(3.0),
            n(1.0),
            n(60.0),
            n(120.0),
            n(2.0),
            n(120.0),
            n(1.0),
            n(30.0),
            e(ErrorKind::DivZero),
            n(3.0),
            n(90.0),
            n(3.0),
            n(1.0)
        ],
        "{got:?}"
    );
}

#[test]
fn lookup_functions() {
    let s = sheet(&[
        ("A1", "apple"),
        ("B1", "1"),
        ("C1", "red"),
        ("A2", "Banana"),
        ("B2", "2"),
        ("C2", "yellow"),
        ("A3", "cherry"),
        ("B3", "3"),
        ("C3", "red"),
        ("E1", "10"),
        ("E2", "20"),
        ("E3", "30"),
        ("F1", "low"),
        ("F2", "mid"),
        ("F3", "high"),
        ("H1", "=VLOOKUP(\"banana\",A1:C3,2,FALSE)"),
        ("H2", "=VLOOKUP(\"cherry\",A1:C3,3,FALSE)"),
        ("H3", "=VLOOKUP(\"grape\",A1:C3,2,FALSE)"),
        ("H4", "=VLOOKUP(25,E1:F3,2)"),
        ("H5", "=VLOOKUP(5,E1:F3,2)"),
        ("H6", "=VLOOKUP(99,E1:F3,2,TRUE)"),
        ("H7", "=VLOOKUP(\"apple\",A1:C3,4,FALSE)"),
        ("H8", "=VLOOKUP(\"apple\",A1:C3,0,FALSE)"),
        ("H9", "=MATCH(\"cherry\",A1:A3,0)"),
        ("H10", "=MATCH(25,E1:E3,1)"),
        ("H11", "=MATCH(25,E1:E3,0)"),
        ("H12", "=INDEX(A1:C3,2,3)"),
        ("H13", "=INDEX(E1:E3,3)"),
        ("H14", "=INDEX(A1:C3,4,1)"),
        ("H15", "=INDEX(A1:C1,2)"),
        ("H16", "=INDEX(F1:F3,MATCH(30,E1:E3,0))"),
        ("H17", "=HLOOKUP(\"Banana\",A1:C3,3,FALSE)"),
        ("H18", "=HLOOKUP(2,B1:C3,2)"),
        ("H19", "=MATCH(30,E1:E3,-1)"),
        ("J1", "1"),
        ("K1", "2"),
        ("L1", "3"),
        ("J2", "a"),
        ("K2", "b"),
        ("L2", "c"),
        ("H20", "=HLOOKUP(2,J1:L2,2,FALSE)"),
        ("H21", "=HLOOKUP(2.5,J1:L2,2)"),
        ("H22", "=HLOOKUP(9,J1:L2,2,FALSE)"),
        ("H23", "=HLOOKUP(2,J1:L2,3)"),
    ]);
    let h = |i: u32| v(&s, &format!("H{i}"));
    assert_eq!(h(1), n(2.0));
    assert_eq!(h(2), t("red"));
    assert_eq!(h(3), e(ErrorKind::Na));
    assert_eq!(h(4), t("mid"), "approximate: last row not above 25");
    assert_eq!(h(5), e(ErrorKind::Na), "approximate: nothing at or below 5");
    assert_eq!(h(6), t("high"));
    assert_eq!(h(7), e(ErrorKind::Ref), "column index past the table");
    assert_eq!(h(8), e(ErrorKind::Value));
    assert_eq!(h(9), n(3.0));
    assert_eq!(h(10), n(2.0));
    assert_eq!(h(11), e(ErrorKind::Na));
    assert_eq!(h(12), t("yellow"));
    assert_eq!(h(13), n(30.0));
    assert_eq!(h(14), e(ErrorKind::Ref));
    assert_eq!(h(15), n(1.0), "INDEX along a single row picks B1");
    assert_eq!(h(16), t("high"));
    assert_eq!(
        h(17),
        e(ErrorKind::Na),
        "HLOOKUP searches the first row, which holds apple/1/red"
    );
    assert_eq!(
        h(18),
        n(2.0),
        "first row 1,red: 2 approximates to the 1 column; row 2 of it is B2"
    );
    assert_eq!(h(20), t("b"));
    assert_eq!(
        h(21),
        t("b"),
        "approximate match takes the last value not above 2.5"
    );
    assert_eq!(h(22), e(ErrorKind::Na));
    assert_eq!(h(23), e(ErrorKind::Ref), "row index past the table");
    assert_eq!(
        h(19),
        e(ErrorKind::Na),
        "descending match with ascending data"
    );
}

#[test]
fn whole_row_and_column_ranges() {
    let s = sheet(&[
        ("A1", "1"),
        ("A2", "2"),
        ("B1", "10"),
        ("C5", "100"),
        ("D1", "=SUM(A:A)"),
        ("D2", "=SUM(1:1)"),
        ("D3", "=SUM(A:B)"),
        ("D4", "=COUNT(5:5)"),
    ]);
    assert_eq!(v(&s, "D1"), n(3.0), "column A: 1 + 2");
    assert_eq!(v(&s, "D2"), n(14.0), "row 1: A1 + B1 + D1 (= 1 + 10 + 3)");
    assert_eq!(v(&s, "D3"), n(13.0), "columns A and B: 1 + 2 + 10");
    assert_eq!(v(&s, "D4"), n(1.0), "row 5 holds one number");
}

#[test]
fn changing_a_precedent_updates_dependents_through_chains_and_ranges() {
    let mut s = sheet(&[
        ("A1", "1"),
        ("B1", "=A1+1"),
        ("C1", "=B1*2"),
        ("A2", "10"),
        ("A3", "20"),
        ("D1", "=SUM(A1:A3)"),
        ("D2", "=D1+C1"),
    ]);
    assert_eq!(
        (v(&s, "B1"), v(&s, "C1"), v(&s, "D1"), v(&s, "D2")),
        (n(2.0), n(4.0), n(31.0), n(35.0))
    );
    s.set_input(k("A1"), "5");
    assert_eq!(
        (v(&s, "B1"), v(&s, "C1"), v(&s, "D1"), v(&s, "D2")),
        (n(6.0), n(12.0), n(35.0), n(47.0))
    );
    // A previously empty cell inside a range starts counting.
    s.set_input(k("A4"), "100");
    assert_eq!(v(&s, "D1"), n(35.0), "A4 is outside A1:A3");
    s.set_input(k("A2"), "");
    assert_eq!(
        v(&s, "D1"),
        n(25.0),
        "clearing a cell removes it from the sum"
    );
    // Text assigned to a number's place changes dependents' results.
    s.set_input(k("A1"), "oops");
    assert_eq!(v(&s, "B1"), e(ErrorKind::Value));
    assert_eq!(v(&s, "C1"), e(ErrorKind::Value));
    assert_eq!(v(&s, "D2"), e(ErrorKind::Value));
    s.set_input(k("A1"), "1");
    assert_eq!(v(&s, "D2"), n(v_num(&s, "D1") + 4.0));
}

#[test]
fn formulas_can_become_values_and_values_formulas() {
    let mut s = sheet(&[("A1", "2"), ("B1", "=A1*10")]);
    s.set_input(k("B1"), "7");
    s.set_input(k("A1"), "99");
    assert_eq!(
        v(&s, "B1"),
        n(7.0),
        "a replaced formula no longer follows its old precedents"
    );
    assert!(!s.is_formula(k("B1")));
    s.set_input(k("A1"), "=B1+1");
    assert_eq!(v(&s, "A1"), n(8.0));
    s.set_input(k("B1"), "3");
    assert_eq!(v(&s, "A1"), n(4.0));
}

#[test]
fn forward_references_and_dependency_order() {
    // A1 reads cells that are defined later in the same batch.
    let s = sheet(&[
        ("A1", "=B1+C1"),
        ("B1", "=C1*2"),
        ("C1", "=D1+1"),
        ("D1", "4"),
    ]);
    assert_eq!(
        (v(&s, "C1"), v(&s, "B1"), v(&s, "A1")),
        (n(5.0), n(10.0), n(15.0))
    );
    // Diamond: D depends on B and C, which both depend on A.
    let mut s = sheet(&[
        ("A1", "1"),
        ("B1", "=A1+1"),
        ("C1", "=A1+2"),
        ("D1", "=B1+C1"),
    ]);
    assert_eq!(v(&s, "D1"), n(5.0));
    s.set_input(k("A1"), "10");
    assert_eq!(v(&s, "D1"), n(23.0));
}

#[test]
fn circular_references_are_detected_and_recoverable() {
    let mut s = sheet(&[("A1", "=B1"), ("B1", "=A1")]);
    assert_eq!(
        (v(&s, "A1"), v(&s, "B1")),
        (e(ErrorKind::Circular), e(ErrorKind::Circular))
    );
    assert_eq!(
        sheet(&[("A1", "=A1+1")]).value(k("A1")),
        e(ErrorKind::Circular),
        "self reference"
    );
    // A range that contains its own formula cell is a cycle, and so is anything reading it.
    let s2 = sheet(&[
        ("A1", "1"),
        ("A2", "=SUM(A1:A3)"),
        ("A3", "2"),
        ("B1", "=A2+1"),
    ]);
    assert_eq!(
        (v(&s2, "A2"), v(&s2, "B1")),
        (e(ErrorKind::Circular), e(ErrorKind::Circular))
    );
    // The same sum placed outside its range is fine.
    let s3 = sheet(&[("A1", "1"), ("A3", "2"), ("B2", "=SUM(A1:A3)")]);
    assert_eq!(v(&s3, "B2"), n(3.0));
    s.set_input(k("B1"), "5");
    assert_eq!(
        (v(&s, "A1"), v(&s, "B1")),
        (n(5.0), n(5.0)),
        "breaking the cycle recovers"
    );
    let mut s = sheet(&[
        ("A1", "=B1"),
        ("B1", "=C1"),
        ("C1", "=A1"),
        ("D1", "=A1+1"),
        ("E1", "=IFERROR(A1,\"x\")"),
    ]);
    assert_eq!(v(&s, "D1"), e(ErrorKind::Circular));
    assert_eq!(v(&s, "E1"), t("x"), "IFERROR catches it");
    s.set_input(k("C1"), "1");
    assert_eq!((v(&s, "A1"), v(&s, "D1")), (n(1.0), n(2.0)));
}

#[test]
fn typed_input_is_interpreted_like_a_spreadsheet() {
    let s = sheet(&[
        ("A1", "42"),
        ("A2", " 3.5 "),
        ("A3", "1,234"),
        ("A4", "12%"),
        ("A5", "$5"),
        ("A6", "TRUE"),
        ("A7", "false"),
        ("A8", "hello"),
        ("A9", "'123"),
        ("A10", "=1+"),
        ("A11", "="),
        ("A12", "#N/A"),
        ("A13", "007"),
        ("A14", "  "),
        ("A15", "1e3"),
        ("A16", "12abc"),
        ("A17", "$1,234.50"),
        ("A18", "-5%"),
    ]);
    assert_eq!(v(&s, "A1"), n(42.0));
    assert_eq!(v(&s, "A2"), n(3.5));
    assert_eq!(v(&s, "A3"), n(1234.0));
    assert_eq!(v(&s, "A4"), n(0.12));
    assert_eq!(
        s.style(k("A4")).fmt,
        NumFmt::Percent { decimals: 0 },
        "typing % picks the percent format"
    );
    assert_eq!(s.display(k("A4")), "12%");
    assert_eq!(v(&s, "A5"), n(5.0));
    assert_eq!(s.style(k("A5")).fmt, NumFmt::Currency { decimals: 2 });
    assert_eq!(s.display(k("A5")), "$5.00");
    assert_eq!(
        (v(&s, "A6"), v(&s, "A7")),
        (Value::Bool(true), Value::Bool(false))
    );
    assert_eq!(v(&s, "A8"), t("hello"));
    assert_eq!(v(&s, "A9"), t("123"), "a leading apostrophe forces text");
    assert_eq!(s.raw(k("A9")), "'123", "and is kept as typed");
    assert_eq!(
        v(&s, "A10"),
        e(ErrorKind::Parse),
        "an invalid formula shows an error"
    );
    assert_eq!(
        s.raw(k("A10")),
        "=1+",
        "but keeps what was typed so it can be fixed"
    );
    assert_eq!(v(&s, "A11"), t("="), "a lone = is text");
    assert_eq!(v(&s, "A12"), e(ErrorKind::Na));
    assert_eq!(v(&s, "A13"), n(7.0));
    assert_eq!(v(&s, "A14"), Value::Empty, "whitespace only is empty");
    assert_eq!(v(&s, "A15"), n(1000.0));
    assert_eq!(v(&s, "A16"), t("12abc"));
    assert_eq!(v(&s, "A17"), n(1234.5));
    assert_eq!(s.display(k("A17")), "$1,234.50");
    assert_eq!(s.style(k("A18")).fmt, NumFmt::Percent { decimals: 0 });
    // Pure formatting does not make a cell "contain" anything.
    assert_eq!(s.used_range().0, 18);
}

#[test]
fn display_uses_the_cell_format_and_general_for_the_rest() {
    let mut s = sheet(&[
        ("A1", "1234.5678"),
        ("A2", "0.256"),
        ("A3", "text"),
        ("A4", "=1/3"),
        ("A5", "=1/0"),
    ]);
    assert_eq!(s.display(k("A1")), "1234.5678");
    s.style_range(k("A1"), k("A1"), |st| {
        st.fmt = NumFmt::Fixed {
            decimals: 2,
            thousands: true,
        }
    });
    assert_eq!(s.display(k("A1")), "1,234.57");
    s.style_range(k("A2"), k("A2"), |st| {
        st.fmt = NumFmt::Percent { decimals: 1 }
    });
    assert_eq!(s.display(k("A2")), "25.6%");
    s.style_range(k("A3"), k("A3"), |st| {
        st.fmt = NumFmt::Fixed {
            decimals: 2,
            thousands: false,
        }
    });
    assert_eq!(s.display(k("A3")), "text", "formats only affect numbers");
    assert_eq!(s.display(k("A4")), "0.333333333333333");
    assert_eq!(s.display(k("A5")), "#DIV/0!");
    assert_eq!(s.display(k("Z1")), "");
    // The format survives editing the content.
    s.set_input(k("A1"), "9999.999");
    assert_eq!(s.display(k("A1")), "10,000.00");
    assert_eq!(v(&s, "A1"), n(9999.999), "the value itself is untouched");
}

#[test]
fn styles_apply_to_ranges_and_clear_independently() {
    let mut s = sheet(&[("A1", "x"), ("B2", "y")]);
    s.style_range(k("A1"), k("B2"), |st| st.bold = true);
    assert!(
        s.style(k("A1")).bold
            && s.style(k("B2")).bold
            && s.style(k("A2")).bold
            && s.style(k("B1")).bold
    );
    assert_eq!(s.used_range(), (2, 2), "styled empty cells count as used");
    s.clear_contents(k("A1"), k("B2"));
    assert_eq!((v(&s, "A1"), v(&s, "B2")), (Value::Empty, Value::Empty));
    assert!(s.style(k("A1")).bold, "clearing contents keeps formatting");
    s.clear_all(k("A1"), k("B2"));
    assert!(!s.style(k("A1")).bold);
    assert_eq!(s.used_range(), (0, 0));
    // Styling a whole column only touches a bounded area.
    s.style_range((0, 0), (MAX_ROWS - 1, 0), |st| st.italic = true);
    assert_eq!(
        s.used_cells().count(),
        0,
        "no cells to style → nothing is created"
    );
}

#[test]
fn undo_and_redo_restore_content_style_and_dependents() {
    let mut s = Sheet::new();
    s.set_input(k("A1"), "1");
    s.set_input(k("B1"), "=A1*10");
    s.style_range(k("A1"), k("A1"), |st| st.bold = true);
    assert!(s.can_undo() && !s.can_redo());
    s.undo(); // style
    assert!(!s.style(k("A1")).bold);
    s.undo(); // B1
    assert_eq!(v(&s, "B1"), Value::Empty);
    s.redo();
    assert_eq!(v(&s, "B1"), n(10.0));
    s.redo();
    assert!(s.style(k("A1")).bold);
    assert!(!s.can_redo());
    // Undo recalculates dependents.
    s.set_input(k("A1"), "5");
    assert_eq!(v(&s, "B1"), n(50.0));
    s.undo();
    assert_eq!(v(&s, "B1"), n(10.0));
    // A new edit drops the redo history.
    s.redo();
    s.undo();
    s.set_input(k("A1"), "7");
    assert!(!s.can_redo());
    // Revision moves on every change, including undo.
    let r = s.revision();
    s.undo();
    assert!(s.revision() > r);
}

#[test]
fn a_batch_is_one_undo_step_and_no_ops_are_not_steps() {
    let mut s = Sheet::new();
    s.batch(|s| {
        s.set_input(k("A1"), "1");
        s.set_input(k("A2"), "2");
        s.set_input(k("A1"), "3"); // touched twice: undo goes back to *before* the batch
    });
    s.undo();
    assert_eq!(s.used_range(), (0, 0));
    assert!(!s.can_undo());
    let r = s.revision();
    s.set_input(k("A1"), "");
    s.clear_contents(k("A1"), k("C3"));
    s.batch(|_| {});
    assert!(
        !s.can_undo() && s.revision() == r,
        "nothing changed, nothing recorded"
    );
    s.set_input(k("A1"), "x");
    s.set_input(k("A1"), "x");
    s.undo();
    assert!(!s.can_undo(), "re-entering the same text isn't a step");
}

#[test]
fn copy_and_paste_shift_relative_references() {
    let mut s = sheet(&[
        ("A1", "1"),
        ("A2", "2"),
        ("B1", "=A1*10"),
        ("B2", "=$A$1+A2"),
        ("C1", "=B1+B$2"),
    ]);
    let clip = s.copy_range(k("B1"), k("C2"));
    assert_eq!((clip.rows(), clip.cols()), (2, 2));
    let area = s.paste(&clip, k("E4"));
    assert_eq!(area, (k("E4"), k("F5")));
    assert_eq!(
        s.raw(k("E4")),
        "=D4*10",
        "A1 moved by (+3 rows, +3 cols) is D4"
    );
    assert_eq!(s.raw(k("E5")), "=$A$1+D5", "absolute parts stay, A2 → D5");
    assert_eq!(s.raw(k("F4")), "=E4+E$2", "mixed refs");
    assert_eq!(s.raw(k("F5")), "", "empty source cells paste as empty");
    // Pasting empties clears what was there.
    s.set_input(k("F5"), "junk");
    let clip = s.copy_range(k("F5"), k("F5"));
    s.set_input(k("F5"), "other");
    s.paste(&s.copy_range(k("H9"), k("H9")), k("F5"));
    assert_eq!(s.raw(k("F5")), "");
    let _ = clip;
    // One undo step for the whole paste.
    s.undo();
    assert_eq!(s.raw(k("F5")), "other");
    s.undo(); // the "other" edit
    s.undo(); // "junk"
    s.undo(); // the paste of B1:C2
    assert_eq!(s.raw(k("E4")), "");
    // Styles travel with the cells.
    let mut s = sheet(&[("A1", "x")]);
    s.style_range(k("A1"), k("A1"), |st| st.bold = true);
    let clip = s.copy_range(k("A1"), k("A1"));
    s.paste(&clip, k("C3"));
    assert!(s.style(k("C3")).bold);
}

#[test]
fn formulas_pasted_off_the_sheet_become_ref_errors() {
    let mut s = sheet(&[("B2", "=A1+1")]);
    let clip = s.copy_range(k("B2"), k("B2"));
    s.paste(&clip, k("A1")); // A1 would now need column -1 / row -1
    assert_eq!(s.raw(k("A1")), "=#REF!+1");
    assert_eq!(v(&s, "A1"), e(ErrorKind::Ref));
}

#[test]
fn fill_down_and_right_copy_the_first_row_or_column() {
    let mut s = sheet(&[("A1", "1"), ("B1", "=A1*2"), ("C1", "x"), ("A2", "junk")]);
    s.fill_down(k("A1"), k("C4"));
    assert_eq!(s.raw(k("B3")), "=A3*2");
    assert_eq!((v(&s, "A4"), v(&s, "C4")), (n(1.0), t("x")));
    assert_eq!(v(&s, "A2"), n(1.0), "fill overwrites");
    s.set_input(k("E1"), "5");
    s.set_input(k("E2"), "=E1+1");
    s.fill_right(k("E1"), k("G2"));
    assert_eq!(s.raw(k("G2")), "=G1+1");
    assert_eq!(v(&s, "F1"), n(5.0));
    s.undo(); // the fill right
    assert_eq!(s.raw(k("G2")), "");
    assert_eq!(s.raw(k("F1")), "");
    s.undo(); // E2
    s.undo(); // E1
    s.undo(); // the fill down
    assert_eq!(
        s.raw(k("A3")),
        "",
        "undo brings back the cells the fill overwrote"
    );
    assert_eq!(s.raw(k("A2")), "junk");
}

#[test]
fn inserting_rows_and_columns_moves_cells_and_rewrites_formulas() {
    let mut s = sheet(&[
        ("A1", "10"),
        ("A2", "20"),
        ("A3", "30"),
        ("B1", "=SUM(A1:A3)"),
        ("B2", "=A2*2"),
        ("C3", "=$A$3+A1"),
    ]);
    s.set_col_width(1, 150.0);
    s.set_row_height(2, 40.0);
    s.insert(Axis::Row, 1, 2); // two new rows before row 2
    assert_eq!(
        (v(&s, "A1"), v(&s, "A2"), v(&s, "A4"), v(&s, "A5")),
        (n(10.0), Value::Empty, n(20.0), n(30.0))
    );
    assert_eq!(
        s.raw(k("B1")),
        "=SUM(A1:A5)",
        "a range spanning the insertion grows"
    );
    assert_eq!(s.raw(k("B4")), "=A4*2");
    assert_eq!(s.raw(k("C5")), "=$A$5+A1");
    assert_eq!(v(&s, "B1"), n(60.0));
    assert_eq!(s.row_height(4), 40.0, "row heights move with their rows");
    assert_eq!(s.col_width(1), 150.0);
    s.insert(Axis::Col, 0, 1);
    assert_eq!(s.raw(k("C1")), "=SUM(B1:B5)");
    assert_eq!(
        s.col_width(2),
        150.0,
        "column widths move with their columns"
    );
    s.undo();
    s.undo();
    assert_eq!(s.raw(k("B1")), "=SUM(A1:A3)");
    assert_eq!(s.row_height(2), 40.0);
    s.redo();
    assert_eq!(s.raw(k("B1")), "=SUM(A1:A5)");
}

#[test]
fn deleting_rows_and_columns_breaks_or_shrinks_references() {
    let mut s = sheet(&[
        ("A1", "1"),
        ("A2", "2"),
        ("A3", "3"),
        ("A4", "4"),
        ("B1", "=SUM(A1:A4)"),
        ("B2", "=A2"),
        ("B3", "=A3+A4"),
        ("B4", "=A1"),
    ]);
    s.delete(Axis::Row, 1, 2); // rows 2 and 3
    assert_eq!((v(&s, "A1"), v(&s, "A2")), (n(1.0), n(4.0)));
    assert_eq!(s.raw(k("B1")), "=SUM(A1:A2)");
    assert_eq!(v(&s, "B1"), n(5.0));
    assert_eq!(s.raw(k("B2")), "=A1", "the old B4 moved up and follows A1");
    assert_eq!(s.used_range(), (2, 2));
    let mut s = sheet(&[("A1", "5"), ("B1", "=A1+1"), ("C1", "=B1*2")]);
    s.delete(Axis::Col, 0, 1);
    assert_eq!(s.raw(k("A1")), "=#REF!+1");
    assert_eq!(v(&s, "A1"), e(ErrorKind::Ref));
    assert_eq!(
        v(&s, "B1"),
        e(ErrorKind::Ref),
        "dependents of a broken formula break too"
    );
    s.undo();
    assert_eq!((v(&s, "B1"), v(&s, "C1")), (n(6.0), n(12.0)));
    // Deleting nothing, or past the data, is harmless.
    let before = s.revision();
    s.delete(Axis::Row, 0, 0);
    assert_eq!(s.revision(), before);
    s.delete(Axis::Row, 500, 10);
    assert_eq!(v(&s, "B1"), n(6.0));
}

#[test]
fn load_replaces_everything_and_clears_history() {
    let mut s = sheet(&[("A1", "old")]);
    let mut cells = std::collections::BTreeMap::new();
    cells.insert(
        k("B2"),
        CellData {
            raw: "=1+1".into(),
            ..Default::default()
        },
    );
    s.load(cells);
    assert_eq!((v(&s, "A1"), v(&s, "B2")), (Value::Empty, n(2.0)));
    assert!(!s.can_undo() && s.revision() == 0);
    let exported = s.export();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[&k("B2")].raw, "=1+1");
}

#[test]
fn performance_smoke_long_dependency_chain_and_wide_ranges() {
    // 2,000-cell chain: each cell adds 1 to the previous; editing the head recomputes all.
    let mut s = Sheet::new();
    s.batch(|s| {
        s.set_input((0, 0), "1");
        for r in 1..2000u32 {
            s.set_input((r, 0), &format!("=A{r}+1"));
        }
    });
    assert_eq!(s.value((1999, 0)), n(2000.0));
    s.set_input((0, 0), "10");
    assert_eq!(s.value((1999, 0)), n(2009.0));
    // A formula over a huge range only visits existing cells.
    s.set_input((0, 2), "=SUM(A:A)");
    assert_eq!(s.value((0, 2)), n((10..=2009).map(f64::from).sum::<f64>()));
    s.set_input((0, 1), "=SUM(1:1048576)");
    assert!(matches!(
        s.value((0, 1)),
        Value::Number(_) | Value::Error(ErrorKind::Circular)
    ));
}
