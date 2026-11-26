//! Financial functions: time value of money, cash-flow analysis, depreciation and securities.
//!
//! Sign convention follows Excel: money paid out is negative, money received is positive.
//! Date arguments are serial numbers (truncated) in the workbook's date system.

use gridcraft_core::date::{datetime_from_serial, days_in_month, is_leap, max_serial, serial_from_ymd};
use gridcraft_core::{CellError, DateSystem, Value};

use crate::util::{R, S, array_numbers, as_array, flatten, has, num, num_val, numbers, opt_bool, opt_num, round_half_away, text, to_sig_digits};
use crate::{Arg, Ctx, FnSpec, VAR};

mod day_count;
mod depreciation;
mod securities;
mod tvm;
pub(crate) use tvm::cumulative_args;

use day_count::{coupdaybs, coupdays, coupdaysnc, coupncd, coupnum, couppcd};
use depreciation::{amordegrc, amorlinc, db, ddb, sln, syd, vdb};
use securities::{
    accrint, accrintm, disc, duration, intrate, mduration, oddfprice, oddfyield, oddlprice, oddlyield, price, pricedisc, pricemat, received, tbilleq,
    tbillprice, tbillyield, yield_, yielddisc, yieldmat,
};
use tvm::{
    cumipmt, cumprinc, dollarde, dollarfr, effect, fv, fvschedule, ipmt, irr, ispmt, mirr, nominal, nper, npv, pduration, pmt, ppmt, pv, rate, rri,
    xirr, xnpv,
};

fn type_arg(args: &[Arg], i: usize) -> R<f64> {
    Ok(if opt_num(args, i, 0.0)? != 0.0 { 1.0 } else { 0.0 })
}

// ---------------------------------------------------------------------------------------------
// Euro conversion
// ---------------------------------------------------------------------------------------------

/// The currencies EUROCONVERT documents: ISO code, the irrevocable conversion rate in units per
/// euro (Council Regulation (EC) No 2866/98 as amended) and the calculation precision in decimals.
const EURO_RATES: &[(&str, f64, i32)] = &[
    ("BEF", 40.3399, 0),
    ("LUF", 40.3399, 0),
    ("DEM", 1.95583, 2),
    ("ESP", 166.386, 0),
    ("FRF", 6.55957, 2),
    ("IEP", 0.787564, 2),
    ("ITL", 1936.27, 0),
    ("NLG", 2.20371, 2),
    ("ATS", 13.7603, 2),
    ("PTE", 200.482, 0),
    ("FIM", 5.94573, 2),
    ("GRD", 340.75, 0),
    ("SIT", 239.64, 2),
    ("EUR", 1.0, 2),
];

fn euro_currency(a: &[Arg], i: usize) -> R<(&'static str, f64, i32)> {
    let code = text(a, i)?;
    let code = code.trim();
    EURO_RATES.iter().copied().find(|(c, _, _)| c.eq_ignore_ascii_case(code)).ok_or(CellError::Value)
}

/// EUROCONVERT(number, source, target, [full_precision], [triangulation_precision]).
fn euroconvert(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let (src, src_rate, _) = euro_currency(a, 1)?;
    let (tgt, tgt_rate, tgt_prec) = euro_currency(a, 2)?;
    let full = opt_bool(a, 3, false)?;
    let triangulation = if has(a, 4) {
        let t = num(a, 4)?.trunc();
        if t < 3.0 {
            return Err(CellError::Value);
        }
        // Beyond 17 significant digits rounding changes nothing.
        Some(t.min(17.0) as usize)
    } else {
        None
    };
    if src == tgt {
        return num_val(x);
    }
    let mut euros = x / src_rate;
    if src != "EUR"
        && let Some(t) = triangulation
    {
        euros = to_sig_digits(euros, t);
    }
    let out = euros * tgt_rate;
    num_val(if full { out } else { round_half_away(out, tgt_prec) })
}

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!(
            "PMT",
            3,
            5,
            Financial,
            S,
            "PMT(rate, nper, pv, [fv], [type])",
            "Periodic payment for a loan or annuity with constant payments and rate.",
            pmt
        ),
        f!("IPMT", 4, 6, Financial, S, "IPMT(rate, per, nper, pv, [fv], [type])", "Interest part of a given period's payment.", ipmt),
        f!("PPMT", 4, 6, Financial, S, "PPMT(rate, per, nper, pv, [fv], [type])", "Principal part of a given period's payment.", ppmt),
        f!("PV", 3, 5, Financial, S, "PV(rate, nper, pmt, [fv], [type])", "Present value of a series of equal future payments.", pv),
        f!("FV", 3, 5, Financial, S, "FV(rate, nper, pmt, [pv], [type])", "Future value of an investment with constant payments and rate.", fv),
        f!("NPER", 3, 5, Financial, S, "NPER(rate, pmt, pv, [fv], [type])", "Number of periods needed to pay off or accumulate an amount.", nper),
        f!(
            "RATE",
            3,
            6,
            Financial,
            S,
            "RATE(nper, pmt, pv, [fv], [type], [guess])",
            "Interest rate per period of an annuity, found iteratively.",
            rate
        ),
        f!(
            "CUMIPMT",
            6,
            6,
            Financial,
            S,
            "CUMIPMT(rate, nper, pv, start_period, end_period, type)",
            "Total interest paid between two periods.",
            cumipmt
        ),
        f!(
            "CUMPRINC",
            6,
            6,
            Financial,
            S,
            "CUMPRINC(rate, nper, pv, start_period, end_period, type)",
            "Total principal repaid between two periods.",
            cumprinc
        ),
        f!(
            "NPV",
            2,
            VAR,
            Financial,
            &[true, false],
            "NPV(rate, value1, [value2], ...)",
            "Net present value of periodic cash flows at a discount rate.",
            npv
        ),
        f!("XNPV", 3, 3, Financial, &[true, false, false], "XNPV(rate, values, dates)", "Net present value of cash flows on specific dates.", xnpv),
        f!("IRR", 1, 2, Financial, &[false, true], "IRR(values, [guess])", "Internal rate of return of periodic cash flows.", irr),
        f!(
            "XIRR",
            2,
            3,
            Financial,
            &[false, false, true],
            "XIRR(values, dates, [guess])",
            "Internal rate of return of cash flows on specific dates.",
            xirr
        ),
        f!(
            "MIRR",
            3,
            3,
            Financial,
            &[false, true, true],
            "MIRR(values, finance_rate, reinvest_rate)",
            "Modified internal rate of return using separate borrow and reinvest rates.",
            mirr
        ),
        f!(
            "FVSCHEDULE",
            2,
            2,
            Financial,
            &[true, false],
            "FVSCHEDULE(principal, schedule)",
            "Future value of a principal after a series of compound rates.",
            fvschedule
        ),
        f!("EFFECT", 2, 2, Financial, S, "EFFECT(nominal_rate, npery)", "Effective annual rate from a nominal rate and compounding periods.", effect),
        f!(
            "NOMINAL",
            2,
            2,
            Financial,
            S,
            "NOMINAL(effect_rate, npery)",
            "Nominal annual rate from an effective rate and compounding periods.",
            nominal
        ),
        f!("ISPMT", 4, 4, Financial, S, "ISPMT(rate, per, nper, pv)", "Interest paid in a period of a loan with even principal payments.", ispmt),
        f!("PDURATION", 3, 3, Financial, S, "PDURATION(rate, pv, fv)", "Periods needed for an investment to grow to a target value.", pduration),
        f!("RRI", 3, 3, Financial, S, "RRI(nper, pv, fv)", "Equivalent interest rate for the growth of an investment.", rri),
        f!(
            "DOLLARDE",
            2,
            2,
            Financial,
            S,
            "DOLLARDE(fractional_dollar, fraction)",
            "Converts a fractional price notation to a decimal number.",
            dollarde
        ),
        f!("DOLLARFR", 2, 2, Financial, S, "DOLLARFR(decimal_dollar, fraction)", "Converts a decimal price to fractional notation.", dollarfr),
        f!("SLN", 3, 3, Financial, S, "SLN(cost, salvage, life)", "Straight-line depreciation for one period.", sln),
        f!("SYD", 4, 4, Financial, S, "SYD(cost, salvage, life, per)", "Sum-of-years'-digits depreciation for a period.", syd),
        f!("DB", 4, 5, Financial, S, "DB(cost, salvage, life, period, [month])", "Fixed-declining-balance depreciation for a period.", db),
        f!(
            "DDB",
            4,
            5,
            Financial,
            S,
            "DDB(cost, salvage, life, period, [factor])",
            "Double-declining-balance (or other factor) depreciation for a period.",
            ddb
        ),
        f!(
            "VDB",
            5,
            7,
            Financial,
            S,
            "VDB(cost, salvage, life, start_period, end_period, [factor], [no_switch])",
            "Declining-balance depreciation over any span of periods, switching to straight line.",
            vdb
        ),
        f!(
            "AMORLINC",
            6,
            7,
            Financial,
            S,
            "AMORLINC(cost, date_purchased, first_period, salvage, period, rate, [basis])",
            "French linear depreciation prorated for the first period.",
            amorlinc
        ),
        f!(
            "AMORDEGRC",
            6,
            7,
            Financial,
            S,
            "AMORDEGRC(cost, date_purchased, first_period, salvage, period, rate, [basis])",
            "French degressive depreciation with a life-based coefficient.",
            amordegrc
        ),
        f!(
            "ACCRINT",
            6,
            8,
            Financial,
            S,
            "ACCRINT(issue, first_interest, settlement, rate, par, frequency, [basis], [calc_method])",
            "Accrued interest of a security paying periodic interest.",
            accrint
        ),
        f!(
            "ACCRINTM",
            3,
            5,
            Financial,
            S,
            "ACCRINTM(issue, settlement, rate, [par], [basis])",
            "Accrued interest of a security paying interest at maturity.",
            accrintm
        ),
        f!(
            "COUPDAYBS",
            3,
            4,
            Financial,
            S,
            "COUPDAYBS(settlement, maturity, frequency, [basis])",
            "Days from the start of the coupon period to settlement.",
            coupdaybs
        ),
        f!(
            "COUPDAYS",
            3,
            4,
            Financial,
            S,
            "COUPDAYS(settlement, maturity, frequency, [basis])",
            "Days in the coupon period that contains settlement.",
            coupdays
        ),
        f!(
            "COUPDAYSNC",
            3,
            4,
            Financial,
            S,
            "COUPDAYSNC(settlement, maturity, frequency, [basis])",
            "Days from settlement to the next coupon date.",
            coupdaysnc
        ),
        f!("COUPNCD", 3, 4, Financial, S, "COUPNCD(settlement, maturity, frequency, [basis])", "Next coupon date after settlement.", coupncd),
        f!(
            "COUPPCD",
            3,
            4,
            Financial,
            S,
            "COUPPCD(settlement, maturity, frequency, [basis])",
            "Previous coupon date on or before settlement.",
            couppcd
        ),
        f!(
            "COUPNUM",
            3,
            4,
            Financial,
            S,
            "COUPNUM(settlement, maturity, frequency, [basis])",
            "Number of coupons payable between settlement and maturity.",
            coupnum
        ),
        f!("DISC", 4, 5, Financial, S, "DISC(settlement, maturity, pr, redemption, [basis])", "Discount rate of a security.", disc),
        f!(
            "PRICEDISC",
            4,
            5,
            Financial,
            S,
            "PRICEDISC(settlement, maturity, discount, redemption, [basis])",
            "Price per 100 face value of a discounted security.",
            pricedisc
        ),
        f!(
            "YIELDDISC",
            4,
            5,
            Financial,
            S,
            "YIELDDISC(settlement, maturity, pr, redemption, [basis])",
            "Annual yield of a discounted security.",
            yielddisc
        ),
        f!(
            "INTRATE",
            4,
            5,
            Financial,
            S,
            "INTRATE(settlement, maturity, investment, redemption, [basis])",
            "Interest rate of a fully invested security.",
            intrate
        ),
        f!(
            "RECEIVED",
            4,
            5,
            Financial,
            S,
            "RECEIVED(settlement, maturity, investment, discount, [basis])",
            "Amount received at maturity for a fully invested security.",
            received
        ),
        f!(
            "TBILLPRICE",
            3,
            3,
            Financial,
            S,
            "TBILLPRICE(settlement, maturity, discount)",
            "Price per 100 face value of a Treasury bill.",
            tbillprice
        ),
        f!("TBILLYIELD", 3, 3, Financial, S, "TBILLYIELD(settlement, maturity, pr)", "Yield of a Treasury bill.", tbillyield),
        f!("TBILLEQ", 3, 3, Financial, S, "TBILLEQ(settlement, maturity, discount)", "Bond-equivalent yield of a Treasury bill.", tbilleq),
        f!(
            "PRICEMAT",
            5,
            6,
            Financial,
            S,
            "PRICEMAT(settlement, maturity, issue, rate, yld, [basis])",
            "Price per 100 face value of a security paying interest at maturity.",
            pricemat
        ),
        f!(
            "YIELDMAT",
            5,
            6,
            Financial,
            S,
            "YIELDMAT(settlement, maturity, issue, rate, pr, [basis])",
            "Annual yield of a security paying interest at maturity.",
            yieldmat
        ),
        f!(
            "PRICE",
            6,
            7,
            Financial,
            S,
            "PRICE(settlement, maturity, rate, yld, redemption, frequency, [basis])",
            "Price per 100 face value of a security paying periodic interest.",
            price
        ),
        f!(
            "YIELD",
            6,
            7,
            Financial,
            S,
            "YIELD(settlement, maturity, rate, pr, redemption, frequency, [basis])",
            "Yield of a security paying periodic interest.",
            yield_
        ),
        f!(
            "DURATION",
            5,
            6,
            Financial,
            S,
            "DURATION(settlement, maturity, coupon, yld, frequency, [basis])",
            "Macaulay duration of a security with periodic interest.",
            duration
        ),
        f!(
            "MDURATION",
            5,
            6,
            Financial,
            S,
            "MDURATION(settlement, maturity, coupon, yld, frequency, [basis])",
            "Modified duration of a security with periodic interest.",
            mduration
        ),
        f!(
            "ODDFPRICE",
            8,
            9,
            Financial,
            S,
            "ODDFPRICE(settlement, maturity, issue, first_coupon, rate, yld, redemption, frequency, [basis])",
            "Price per 100 face value of a security with an odd first period.",
            oddfprice
        ),
        f!(
            "ODDFYIELD",
            8,
            9,
            Financial,
            S,
            "ODDFYIELD(settlement, maturity, issue, first_coupon, rate, pr, redemption, frequency, [basis])",
            "Yield of a security with an odd first period.",
            oddfyield
        ),
        f!(
            "ODDLPRICE",
            7,
            8,
            Financial,
            S,
            "ODDLPRICE(settlement, maturity, last_interest, rate, yld, redemption, frequency, [basis])",
            "Price per 100 face value of a security with an odd last period.",
            oddlprice
        ),
        f!(
            "ODDLYIELD",
            7,
            8,
            Financial,
            S,
            "ODDLYIELD(settlement, maturity, last_interest, rate, pr, redemption, frequency, [basis])",
            "Yield of a security with an odd last period.",
            oddlyield
        ),
        f!(
            "EUROCONVERT",
            3,
            5,
            AddIn,
            S,
            "EUROCONVERT(number, source, target, [full_precision], [triangulation_precision])",
            "Converts between the euro and the former euro member currencies at the fixed rates.",
            euroconvert
        ),
    ]
}

#[cfg(test)]
mod tests;
