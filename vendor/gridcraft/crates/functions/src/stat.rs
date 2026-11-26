//! Statistical functions: aggregates, order statistics, regression and probability
//! distributions (with their inverses).

use gridcraft_core::{CellError, Value};

use crate::special::{
    erf, gamma, invert, ln_beta, ln_gamma, norm_cdf, norm_inv, norm_pdf, reg_gamma_p, reg_gamma_q, reg_gamma_q_front, reg_inc_beta,
    reg_inc_beta_front,
};
use crate::util::{A, R, S, arg, array_numbers, array_val, as_array, boolean, has, num, num_val, numbers, numbers_a, opt_num};
use crate::{Arg, Ctx, FnSpec, VAR};

mod descriptive;
mod distributions;
mod regression;

pub use descriptive::{aggregate_values, aggregate_values_k};
use descriptive::{
    arg_nums, arg_pairs, avedev, correl, count_impl, counta_impl, countblank, devsq, frequency, geomean, harmean, intercept, kurt, large, max_of,
    mean, median, min_of, mode_mult, mode_numbers, mode_sngl, percentile_exc, percentile_inc, percentrank_impl, prob, quartile_exc, quartile_inc,
    rank_impl, skew, skew_p, slope, small, standardize, trimmean, var_p, var_s,
};
use distributions::{
    beta_dist, beta_inv, betadist_legacy, binom_dist, binom_dist_range, binom_inv, chisq_dist, chisq_dist_rt, chisq_inv, chisq_inv_rt, chisq_test,
    confidence_norm, confidence_t, expon_dist, f_dist, f_dist_rt, f_inv, f_inv_rt, f_test, fisher, gamma_dist, gamma_fn, gamma_inv, gammaln, gauss,
    hypgeom_dist, lognorm_dist, lognorm_inv, negbinom_dist, norm_dist, norm_inv_fn, norm_s_dist, norm_s_inv, poisson_dist, t_dist, t_dist_2t,
    t_dist_rt, t_inv, t_inv_2t, t_test, tdist_legacy, weibull_dist, z_test,
};
#[cfg(test)]
use regression::{MAX_REGRESSORS, check_regression_size};
use regression::{covariance, forecast, linest_impl, steyx, trend_impl};

type C<'a> = &'a mut dyn Ctx;

// ---------------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------------

const S_ARR_K: &[bool] = &[false, true];
const S_RANK: &[bool] = &[true, false, true];
const S_PRANK: &[bool] = &[false, true, true];
const S_FORECAST: &[bool] = &[true, false, false];
const S_PROB: &[bool] = &[false, false, true, true];
const S_TTEST: &[bool] = &[false, false, true, true];

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!("AVERAGE", 1, VAR, Statistical, A, "AVERAGE(number1, [number2], ...)", "Arithmetic mean of the numbers.", |a: &[Arg], _c: C| num_val(
            mean(&numbers(a)?)?
        )),
        f!(
            "AVERAGEA",
            1,
            VAR,
            Statistical,
            A,
            "AVERAGEA(value1, [value2], ...)",
            "Mean counting text as 0 and logical values as 1 or 0.",
            |a: &[Arg], _c: C| num_val(mean(&numbers_a(a)?)?)
        ),
        f!("MEDIAN", 1, VAR, Statistical, A, "MEDIAN(number1, [number2], ...)", "Middle value of the numbers.", |a: &[Arg], _c: C| num_val(median(
            numbers(a)?
        )?)),
        f!("MODE", 1, VAR, Compatibility, A, "MODE(number1, [number2], ...)", "Most frequently occurring number.", |a: &[Arg], _c: C| num_val(
            mode_sngl(&mode_numbers(a)?)?
        )),
        f!("MODE.SNGL", 1, VAR, Statistical, A, "MODE.SNGL(number1, [number2], ...)", "Most frequently occurring number.", |a: &[Arg], _c: C| {
            num_val(mode_sngl(&mode_numbers(a)?)?)
        }),
        f!("MODE.MULT", 1, VAR, Statistical, A, "MODE.MULT(number1, [number2], ...)", "Vertical array of all the most frequent numbers.", mode_mult),
        f!("STDEV", 1, VAR, Compatibility, A, "STDEV(number1, [number2], ...)", "Sample standard deviation.", |a: &[Arg], _c: C| num_val(
            var_s(&numbers(a)?)?.sqrt()
        )),
        f!("STDEV.S", 1, VAR, Statistical, A, "STDEV.S(number1, [number2], ...)", "Sample standard deviation.", |a: &[Arg], _c: C| num_val(
            var_s(&numbers(a)?)?.sqrt()
        )),
        f!("STDEV.P", 1, VAR, Statistical, A, "STDEV.P(number1, [number2], ...)", "Population standard deviation.", |a: &[Arg], _c: C| num_val(
            var_p(&numbers(a)?)?.sqrt()
        )),
        f!("STDEVP", 1, VAR, Compatibility, A, "STDEVP(number1, [number2], ...)", "Population standard deviation.", |a: &[Arg], _c: C| num_val(
            var_p(&numbers(a)?)?.sqrt()
        )),
        f!(
            "STDEVA",
            1,
            VAR,
            Statistical,
            A,
            "STDEVA(value1, [value2], ...)",
            "Sample standard deviation counting text as 0 and logicals as 1/0.",
            |a: &[Arg], _c: C| { num_val(var_s(&numbers_a(a)?)?.sqrt()) }
        ),
        f!(
            "STDEVPA",
            1,
            VAR,
            Statistical,
            A,
            "STDEVPA(value1, [value2], ...)",
            "Population standard deviation counting text as 0 and logicals as 1/0.",
            |a: &[Arg], _c: C| { num_val(var_p(&numbers_a(a)?)?.sqrt()) }
        ),
        f!("VAR", 1, VAR, Compatibility, A, "VAR(number1, [number2], ...)", "Sample variance.", |a: &[Arg], _c: C| num_val(var_s(&numbers(a)?)?)),
        f!("VAR.S", 1, VAR, Statistical, A, "VAR.S(number1, [number2], ...)", "Sample variance.", |a: &[Arg], _c: C| num_val(var_s(&numbers(a)?)?)),
        f!("VAR.P", 1, VAR, Statistical, A, "VAR.P(number1, [number2], ...)", "Population variance.", |a: &[Arg], _c: C| num_val(var_p(&numbers(
            a
        )?)?)),
        f!("VARP", 1, VAR, Compatibility, A, "VARP(number1, [number2], ...)", "Population variance.", |a: &[Arg], _c: C| num_val(var_p(&numbers(
            a
        )?)?)),
        f!(
            "VARA",
            1,
            VAR,
            Statistical,
            A,
            "VARA(value1, [value2], ...)",
            "Sample variance counting text as 0 and logicals as 1/0.",
            |a: &[Arg], _c: C| num_val(var_s(&numbers_a(a)?)?)
        ),
        f!(
            "VARPA",
            1,
            VAR,
            Statistical,
            A,
            "VARPA(value1, [value2], ...)",
            "Population variance counting text as 0 and logicals as 1/0.",
            |a: &[Arg], _c: C| num_val(var_p(&numbers_a(a)?)?)
        ),
        f!("MIN", 0, VAR, Statistical, A, "MIN(number1, [number2], ...)", "Smallest number (0 when there are none).", |a: &[Arg], _c: C| num_val(
            min_of(&numbers(a)?)
        )),
        f!(
            "MINA",
            1,
            VAR,
            Statistical,
            A,
            "MINA(value1, [value2], ...)",
            "Smallest value counting text as 0 and logicals as 1/0.",
            |a: &[Arg], _c: C| num_val(min_of(&numbers_a(a)?))
        ),
        f!("MAX", 1, VAR, Statistical, A, "MAX(number1, [number2], ...)", "Largest number (0 when there are none).", |a: &[Arg], _c: C| num_val(
            max_of(&numbers(a)?)
        )),
        f!(
            "MAXA",
            1,
            VAR,
            Statistical,
            A,
            "MAXA(value1, [value2], ...)",
            "Largest value counting text as 0 and logicals as 1/0.",
            |a: &[Arg], _c: C| num_val(max_of(&numbers_a(a)?))
        ),
        f!("LARGE", 2, 2, Statistical, S_ARR_K, "LARGE(array, k)", "The k-th largest number in a data set.", large),
        f!("SMALL", 2, 2, Statistical, S_ARR_K, "SMALL(array, k)", "The k-th smallest number in a data set.", small),
        f!("RANK", 2, 3, Compatibility, S_RANK, "RANK(number, ref, [order])", "Rank of a number within a list.", |a: &[Arg], _c: C| rank_impl(
            a, false
        )),
        f!(
            "RANK.EQ",
            2,
            3,
            Statistical,
            S_RANK,
            "RANK.EQ(number, ref, [order])",
            "Rank of a number within a list; ties share the top rank.",
            |a: &[Arg], _c: C| rank_impl(a, false)
        ),
        f!(
            "RANK.AVG",
            2,
            3,
            Statistical,
            S_RANK,
            "RANK.AVG(number, ref, [order])",
            "Rank of a number within a list; ties get the average rank.",
            |a: &[Arg], _c: C| { rank_impl(a, true) }
        ),
        f!("PERCENTILE", 2, 2, Compatibility, S_ARR_K, "PERCENTILE(array, k)", "k-th percentile, k from 0 to 1 inclusive.", |a: &[Arg], _c: C| {
            num_val(percentile_inc(arg_nums(a, 0)?, num(a, 1)?)?)
        }),
        f!(
            "PERCENTILE.INC",
            2,
            2,
            Statistical,
            S_ARR_K,
            "PERCENTILE.INC(array, k)",
            "k-th percentile, k from 0 to 1 inclusive.",
            |a: &[Arg], _c: C| num_val(percentile_inc(arg_nums(a, 0)?, num(a, 1)?)?)
        ),
        f!(
            "PERCENTILE.EXC",
            2,
            2,
            Statistical,
            S_ARR_K,
            "PERCENTILE.EXC(array, k)",
            "k-th percentile, k strictly between 0 and 1.",
            |a: &[Arg], _c: C| num_val(percentile_exc(arg_nums(a, 0)?, num(a, 1)?)?)
        ),
        f!("QUARTILE", 2, 2, Compatibility, S_ARR_K, "QUARTILE(array, quart)", "Quartile (0–4) of a data set.", |a: &[Arg], _c: C| num_val(
            quartile_inc(arg_nums(a, 0)?, num(a, 1)?)?
        )),
        f!(
            "QUARTILE.INC",
            2,
            2,
            Statistical,
            S_ARR_K,
            "QUARTILE.INC(array, quart)",
            "Quartile (0–4) of a data set, inclusive method.",
            |a: &[Arg], _c: C| num_val(quartile_inc(arg_nums(a, 0)?, num(a, 1)?)?)
        ),
        f!(
            "QUARTILE.EXC",
            2,
            2,
            Statistical,
            S_ARR_K,
            "QUARTILE.EXC(array, quart)",
            "Quartile (1–3) of a data set, exclusive method.",
            |a: &[Arg], _c: C| num_val(quartile_exc(arg_nums(a, 0)?, num(a, 1)?)?)
        ),
        f!(
            "PERCENTRANK",
            2,
            3,
            Compatibility,
            S_PRANK,
            "PERCENTRANK(array, x, [significance])",
            "Relative standing of a value as a fraction of the data set.",
            |a: &[Arg], _c: C| { percentrank_impl(a, false) }
        ),
        f!(
            "PERCENTRANK.INC",
            2,
            3,
            Statistical,
            S_PRANK,
            "PERCENTRANK.INC(array, x, [significance])",
            "Relative standing of a value from 0 to 1 inclusive.",
            |a: &[Arg], _c: C| percentrank_impl(a, false)
        ),
        f!(
            "PERCENTRANK.EXC",
            2,
            3,
            Statistical,
            S_PRANK,
            "PERCENTRANK.EXC(array, x, [significance])",
            "Relative standing of a value strictly between 0 and 1.",
            |a: &[Arg], _c: C| percentrank_impl(a, true)
        ),
        f!("COUNT", 1, VAR, Statistical, A, "COUNT(value1, [value2], ...)", "Counts the numbers.", |a: &[Arg], _c: C| num_val(count_impl(a))),
        f!("COUNTA", 1, VAR, Statistical, A, "COUNTA(value1, [value2], ...)", "Counts the non-empty values.", |a: &[Arg], _c: C| num_val(
            counta_impl(a)
        )),
        f!("COUNTBLANK", 1, 1, Statistical, A, "COUNTBLANK(range)", "Counts empty cells and empty text in a range.", countblank),
        f!("CORREL", 2, 2, Statistical, A, "CORREL(array1, array2)", "Correlation coefficient of two data sets.", |a: &[Arg], _c: C| {
            let (x, y) = arg_pairs(a, 0, 1)?;
            num_val(correl(&x, &y)?)
        }),
        f!("PEARSON", 2, 2, Statistical, A, "PEARSON(array1, array2)", "Pearson product-moment correlation coefficient.", |a: &[Arg], _c: C| {
            let (x, y) = arg_pairs(a, 0, 1)?;
            num_val(correl(&x, &y)?)
        }),
        f!("RSQ", 2, 2, Statistical, A, "RSQ(known_ys, known_xs)", "Square of the correlation coefficient.", |a: &[Arg], _c: C| {
            let (y, x) = arg_pairs(a, 0, 1)?;
            num_val(correl(&x, &y)?.powi(2))
        }),
        f!("SLOPE", 2, 2, Statistical, A, "SLOPE(known_ys, known_xs)", "Slope of the least-squares regression line.", |a: &[Arg], _c: C| {
            let (y, x) = arg_pairs(a, 0, 1)?;
            if y.is_empty() {
                return Err(CellError::Div0);
            }
            num_val(slope(&y, &x)?)
        }),
        f!(
            "INTERCEPT",
            2,
            2,
            Statistical,
            A,
            "INTERCEPT(known_ys, known_xs)",
            "Y-intercept of the least-squares regression line.",
            |a: &[Arg], _c: C| {
                let (y, x) = arg_pairs(a, 0, 1)?;
                if y.is_empty() {
                    return Err(CellError::Div0);
                }
                num_val(intercept(&y, &x)?)
            }
        ),
        f!("STEYX", 2, 2, Statistical, A, "STEYX(known_ys, known_xs)", "Standard error of predicted y values in a regression.", steyx),
        f!("FORECAST", 3, 3, Compatibility, S_FORECAST, "FORECAST(x, known_ys, known_xs)", "Predicts a value along a linear trend.", forecast),
        f!(
            "FORECAST.LINEAR",
            3,
            3,
            Statistical,
            S_FORECAST,
            "FORECAST.LINEAR(x, known_ys, known_xs)",
            "Predicts a value along a linear trend.",
            forecast
        ),
        f!(
            "TREND",
            1,
            4,
            Statistical,
            A,
            "TREND(known_ys, [known_xs], [new_xs], [const])",
            "Values along a least-squares linear fit.",
            |a: &[Arg], _c: C| trend_impl(a, false)
        ),
        f!(
            "GROWTH",
            1,
            4,
            Statistical,
            A,
            "GROWTH(known_ys, [known_xs], [new_xs], [const])",
            "Values along a fitted exponential growth curve.",
            |a: &[Arg], _c: C| { trend_impl(a, true) }
        ),
        f!(
            "LINEST",
            1,
            4,
            Statistical,
            A,
            "LINEST(known_ys, [known_xs], [const], [stats])",
            "Least-squares line coefficients and optional regression statistics.",
            |a: &[Arg], _c: C| { linest_impl(a, false) }
        ),
        f!(
            "LOGEST",
            1,
            4,
            Statistical,
            A,
            "LOGEST(known_ys, [known_xs], [const], [stats])",
            "Exponential curve coefficients and optional regression statistics.",
            |a: &[Arg], _c: C| { linest_impl(a, true) }
        ),
        f!("COVARIANCE.P", 2, 2, Statistical, A, "COVARIANCE.P(array1, array2)", "Population covariance of paired values.", |a: &[Arg], _c: C| {
            covariance(a, false)
        }),
        f!("COVARIANCE.S", 2, 2, Statistical, A, "COVARIANCE.S(array1, array2)", "Sample covariance of paired values.", |a: &[Arg], _c: C| {
            covariance(a, true)
        }),
        f!("COVAR", 2, 2, Compatibility, A, "COVAR(array1, array2)", "Population covariance of paired values.", |a: &[Arg], _c: C| covariance(
            a, false
        )),
        f!("DEVSQ", 1, VAR, Statistical, A, "DEVSQ(number1, [number2], ...)", "Sum of squared deviations from the mean.", |a: &[Arg], _c: C| {
            let v = numbers(a)?;
            if v.is_empty() {
                return Err(CellError::Num);
            }
            num_val(devsq(&v))
        }),
        f!("AVEDEV", 1, VAR, Statistical, A, "AVEDEV(number1, [number2], ...)", "Average absolute deviation from the mean.", |a: &[Arg], _c: C| {
            num_val(avedev(&numbers(a)?)?)
        }),
        f!("GEOMEAN", 1, VAR, Statistical, A, "GEOMEAN(number1, [number2], ...)", "Geometric mean of positive numbers.", |a: &[Arg], _c: C| num_val(
            geomean(&numbers(a)?)?
        )),
        f!("HARMEAN", 1, VAR, Statistical, A, "HARMEAN(number1, [number2], ...)", "Harmonic mean of positive numbers.", |a: &[Arg], _c: C| num_val(
            harmean(&numbers(a)?)?
        )),
        f!("TRIMMEAN", 2, 2, Statistical, S_ARR_K, "TRIMMEAN(array, percent)", "Mean after trimming a fraction of points from both ends.", trimmean),
        f!("KURT", 1, VAR, Statistical, A, "KURT(number1, [number2], ...)", "Excess kurtosis of a sample.", |a: &[Arg], _c: C| num_val(kurt(
            &numbers(a)?
        )?)),
        f!("SKEW", 1, VAR, Statistical, A, "SKEW(number1, [number2], ...)", "Sample skewness.", |a: &[Arg], _c: C| num_val(skew(&numbers(a)?)?)),
        f!("SKEW.P", 1, VAR, Statistical, A, "SKEW.P(number1, [number2], ...)", "Population skewness.", |a: &[Arg], _c: C| num_val(skew_p(
            &numbers(a)?
        )?)),
        f!("STANDARDIZE", 3, 3, Statistical, S, "STANDARDIZE(x, mean, standard_dev)", "Z-score of a value.", standardize),
        f!(
            "FREQUENCY",
            2,
            2,
            Statistical,
            A,
            "FREQUENCY(data_array, bins_array)",
            "Counts how many values fall into each bin, as a vertical array.",
            frequency
        ),
        f!(
            "PROB",
            3,
            4,
            Statistical,
            S_PROB,
            "PROB(x_range, prob_range, lower_limit, [upper_limit])",
            "Probability that values fall between two limits.",
            prob
        ),
        f!(
            "NORM.DIST",
            4,
            4,
            Statistical,
            S,
            "NORM.DIST(x, mean, standard_dev, cumulative)",
            "Normal distribution density or cumulative probability.",
            norm_dist
        ),
        f!(
            "NORMDIST",
            4,
            4,
            Compatibility,
            S,
            "NORMDIST(x, mean, standard_dev, cumulative)",
            "Normal distribution density or cumulative probability.",
            norm_dist
        ),
        f!(
            "NORM.INV",
            3,
            3,
            Statistical,
            S,
            "NORM.INV(probability, mean, standard_dev)",
            "Inverse of the normal cumulative distribution.",
            norm_inv_fn
        ),
        f!(
            "NORMINV",
            3,
            3,
            Compatibility,
            S,
            "NORMINV(probability, mean, standard_dev)",
            "Inverse of the normal cumulative distribution.",
            norm_inv_fn
        ),
        f!("NORM.S.DIST", 2, 2, Statistical, S, "NORM.S.DIST(z, cumulative)", "Standard normal density or cumulative probability.", norm_s_dist),
        f!("NORMSDIST", 1, 1, Compatibility, S, "NORMSDIST(z)", "Standard normal cumulative probability.", norm_s_dist),
        f!("NORM.S.INV", 1, 1, Statistical, S, "NORM.S.INV(probability)", "Inverse of the standard normal cumulative distribution.", norm_s_inv),
        f!("NORMSINV", 1, 1, Compatibility, S, "NORMSINV(probability)", "Inverse of the standard normal cumulative distribution.", norm_s_inv),
        f!("T.DIST", 3, 3, Statistical, S, "T.DIST(x, deg_freedom, cumulative)", "Student's t density or left-tailed probability.", t_dist),
        f!("T.DIST.2T", 2, 2, Statistical, S, "T.DIST.2T(x, deg_freedom)", "Two-tailed Student's t probability.", t_dist_2t),
        f!("T.DIST.RT", 2, 2, Statistical, S, "T.DIST.RT(x, deg_freedom)", "Right-tailed Student's t probability.", t_dist_rt),
        f!("TDIST", 3, 3, Compatibility, S, "TDIST(x, deg_freedom, tails)", "One- or two-tailed Student's t probability.", tdist_legacy),
        f!("T.INV", 2, 2, Statistical, S, "T.INV(probability, deg_freedom)", "Left-tailed inverse of Student's t distribution.", t_inv),
        f!("T.INV.2T", 2, 2, Statistical, S, "T.INV.2T(probability, deg_freedom)", "Two-tailed inverse of Student's t distribution.", t_inv_2t),
        f!("TINV", 2, 2, Compatibility, S, "TINV(probability, deg_freedom)", "Two-tailed inverse of Student's t distribution.", t_inv_2t),
        f!("T.TEST", 4, 4, Statistical, S_TTEST, "T.TEST(array1, array2, tails, type)", "Probability from Student's t-test.", t_test),
        f!("TTEST", 4, 4, Compatibility, S_TTEST, "TTEST(array1, array2, tails, type)", "Probability from Student's t-test.", t_test),
        f!(
            "CHISQ.DIST",
            3,
            3,
            Statistical,
            S,
            "CHISQ.DIST(x, deg_freedom, cumulative)",
            "Chi-squared density or left-tailed probability.",
            chisq_dist
        ),
        f!("CHISQ.DIST.RT", 2, 2, Statistical, S, "CHISQ.DIST.RT(x, deg_freedom)", "Right-tailed chi-squared probability.", chisq_dist_rt),
        f!("CHIDIST", 2, 2, Compatibility, S, "CHIDIST(x, deg_freedom)", "Right-tailed chi-squared probability.", chisq_dist_rt),
        f!(
            "CHISQ.INV",
            2,
            2,
            Statistical,
            S,
            "CHISQ.INV(probability, deg_freedom)",
            "Inverse of the left-tailed chi-squared distribution.",
            chisq_inv
        ),
        f!(
            "CHISQ.INV.RT",
            2,
            2,
            Statistical,
            S,
            "CHISQ.INV.RT(probability, deg_freedom)",
            "Inverse of the right-tailed chi-squared distribution.",
            chisq_inv_rt
        ),
        f!(
            "CHIINV",
            2,
            2,
            Compatibility,
            S,
            "CHIINV(probability, deg_freedom)",
            "Inverse of the right-tailed chi-squared distribution.",
            chisq_inv_rt
        ),
        f!("CHISQ.TEST", 2, 2, Statistical, A, "CHISQ.TEST(actual_range, expected_range)", "Chi-squared test of independence.", chisq_test),
        f!("CHITEST", 2, 2, Compatibility, A, "CHITEST(actual_range, expected_range)", "Chi-squared test of independence.", chisq_test),
        f!(
            "F.DIST",
            4,
            4,
            Statistical,
            S,
            "F.DIST(x, deg_freedom1, deg_freedom2, cumulative)",
            "F distribution density or left-tailed probability.",
            f_dist
        ),
        f!("F.DIST.RT", 3, 3, Statistical, S, "F.DIST.RT(x, deg_freedom1, deg_freedom2)", "Right-tailed F probability.", f_dist_rt),
        f!("FDIST", 3, 3, Compatibility, S, "FDIST(x, deg_freedom1, deg_freedom2)", "Right-tailed F probability.", f_dist_rt),
        f!("F.INV", 3, 3, Statistical, S, "F.INV(probability, deg_freedom1, deg_freedom2)", "Inverse of the left-tailed F distribution.", f_inv),
        f!(
            "F.INV.RT",
            3,
            3,
            Statistical,
            S,
            "F.INV.RT(probability, deg_freedom1, deg_freedom2)",
            "Inverse of the right-tailed F distribution.",
            f_inv_rt
        ),
        f!("FINV", 3, 3, Compatibility, S, "FINV(probability, deg_freedom1, deg_freedom2)", "Inverse of the right-tailed F distribution.", f_inv_rt),
        f!("F.TEST", 2, 2, Statistical, A, "F.TEST(array1, array2)", "Two-tailed probability that two variances do not differ.", f_test),
        f!("FTEST", 2, 2, Compatibility, A, "FTEST(array1, array2)", "Two-tailed probability that two variances do not differ.", f_test),
        f!("BINOM.DIST", 4, 4, Statistical, S, "BINOM.DIST(number_s, trials, probability_s, cumulative)", "Binomial probability.", binom_dist),
        f!("BINOMDIST", 4, 4, Compatibility, S, "BINOMDIST(number_s, trials, probability_s, cumulative)", "Binomial probability.", binom_dist),
        f!(
            "BINOM.DIST.RANGE",
            3,
            4,
            Statistical,
            S,
            "BINOM.DIST.RANGE(trials, probability_s, number_s, [number_s2])",
            "Probability of a number of successes falling in a range.",
            binom_dist_range
        ),
        f!(
            "BINOM.INV",
            3,
            3,
            Statistical,
            S,
            "BINOM.INV(trials, probability_s, alpha)",
            "Smallest success count whose cumulative binomial probability reaches alpha.",
            binom_inv
        ),
        f!(
            "CRITBINOM",
            3,
            3,
            Compatibility,
            S,
            "CRITBINOM(trials, probability_s, alpha)",
            "Smallest success count whose cumulative binomial probability reaches alpha.",
            binom_inv
        ),
        f!("POISSON.DIST", 3, 3, Statistical, S, "POISSON.DIST(x, mean, cumulative)", "Poisson probability.", poisson_dist),
        f!("POISSON", 3, 3, Compatibility, S, "POISSON(x, mean, cumulative)", "Poisson probability.", poisson_dist),
        f!("EXPON.DIST", 3, 3, Statistical, S, "EXPON.DIST(x, lambda, cumulative)", "Exponential distribution density or probability.", expon_dist),
        f!("EXPONDIST", 3, 3, Compatibility, S, "EXPONDIST(x, lambda, cumulative)", "Exponential distribution density or probability.", expon_dist),
        f!("GAMMA", 1, 1, Statistical, S, "GAMMA(number)", "The gamma function.", gamma_fn),
        f!("GAMMA.DIST", 4, 4, Statistical, S, "GAMMA.DIST(x, alpha, beta, cumulative)", "Gamma distribution density or probability.", gamma_dist),
        f!("GAMMADIST", 4, 4, Compatibility, S, "GAMMADIST(x, alpha, beta, cumulative)", "Gamma distribution density or probability.", gamma_dist),
        f!("GAMMA.INV", 3, 3, Statistical, S, "GAMMA.INV(probability, alpha, beta)", "Inverse of the gamma cumulative distribution.", gamma_inv),
        f!("GAMMAINV", 3, 3, Compatibility, S, "GAMMAINV(probability, alpha, beta)", "Inverse of the gamma cumulative distribution.", gamma_inv),
        f!("GAMMALN", 1, 1, Statistical, S, "GAMMALN(x)", "Natural log of the gamma function.", gammaln),
        f!("GAMMALN.PRECISE", 1, 1, Statistical, S, "GAMMALN.PRECISE(x)", "Natural log of the gamma function.", gammaln),
        f!(
            "BETA.DIST",
            4,
            6,
            Statistical,
            S,
            "BETA.DIST(x, alpha, beta, cumulative, [A], [B])",
            "Beta distribution density or probability.",
            beta_dist
        ),
        f!("BETADIST", 3, 5, Compatibility, S, "BETADIST(x, alpha, beta, [A], [B])", "Cumulative beta probability.", betadist_legacy),
        f!(
            "BETA.INV",
            3,
            5,
            Statistical,
            S,
            "BETA.INV(probability, alpha, beta, [A], [B])",
            "Inverse of the cumulative beta distribution.",
            beta_inv
        ),
        f!(
            "BETAINV",
            3,
            5,
            Compatibility,
            S,
            "BETAINV(probability, alpha, beta, [A], [B])",
            "Inverse of the cumulative beta distribution.",
            beta_inv
        ),
        f!(
            "LOGNORM.DIST",
            4,
            4,
            Statistical,
            S,
            "LOGNORM.DIST(x, mean, standard_dev, cumulative)",
            "Lognormal density or probability.",
            lognorm_dist
        ),
        f!("LOGNORMDIST", 3, 3, Compatibility, S, "LOGNORMDIST(x, mean, standard_dev)", "Cumulative lognormal probability.", lognorm_dist),
        f!(
            "LOGNORM.INV",
            3,
            3,
            Statistical,
            S,
            "LOGNORM.INV(probability, mean, standard_dev)",
            "Inverse of the cumulative lognormal distribution.",
            lognorm_inv
        ),
        f!(
            "LOGINV",
            3,
            3,
            Compatibility,
            S,
            "LOGINV(probability, mean, standard_dev)",
            "Inverse of the cumulative lognormal distribution.",
            lognorm_inv
        ),
        f!("WEIBULL.DIST", 4, 4, Statistical, S, "WEIBULL.DIST(x, alpha, beta, cumulative)", "Weibull density or probability.", weibull_dist),
        f!("WEIBULL", 4, 4, Compatibility, S, "WEIBULL(x, alpha, beta, cumulative)", "Weibull density or probability.", weibull_dist),
        f!(
            "HYPGEOM.DIST",
            5,
            5,
            Statistical,
            S,
            "HYPGEOM.DIST(sample_s, number_sample, population_s, number_pop, cumulative)",
            "Hypergeometric probability.",
            hypgeom_dist
        ),
        f!(
            "HYPGEOMDIST",
            4,
            4,
            Compatibility,
            S,
            "HYPGEOMDIST(sample_s, number_sample, population_s, number_pop)",
            "Hypergeometric probability of an exact count.",
            hypgeom_dist
        ),
        f!(
            "NEGBINOM.DIST",
            4,
            4,
            Statistical,
            S,
            "NEGBINOM.DIST(number_f, number_s, probability_s, cumulative)",
            "Negative binomial probability.",
            negbinom_dist
        ),
        f!(
            "NEGBINOMDIST",
            3,
            3,
            Compatibility,
            S,
            "NEGBINOMDIST(number_f, number_s, probability_s)",
            "Negative binomial probability.",
            negbinom_dist
        ),
        f!(
            "CONFIDENCE",
            3,
            3,
            Compatibility,
            S,
            "CONFIDENCE(alpha, standard_dev, size)",
            "Confidence interval half-width using the normal distribution.",
            confidence_norm
        ),
        f!(
            "CONFIDENCE.NORM",
            3,
            3,
            Statistical,
            S,
            "CONFIDENCE.NORM(alpha, standard_dev, size)",
            "Confidence interval half-width using the normal distribution.",
            confidence_norm
        ),
        f!(
            "CONFIDENCE.T",
            3,
            3,
            Statistical,
            S,
            "CONFIDENCE.T(alpha, standard_dev, size)",
            "Confidence interval half-width using Student's t distribution.",
            confidence_t
        ),
        f!("Z.TEST", 2, 3, Statistical, S_PRANK, "Z.TEST(array, x, [sigma])", "One-tailed probability of a z-test.", z_test),
        f!("ZTEST", 2, 3, Compatibility, S_PRANK, "ZTEST(array, x, [sigma])", "One-tailed probability of a z-test.", z_test),
        f!("FISHER", 1, 1, Statistical, S, "FISHER(x)", "Fisher transformation.", fisher),
        f!("FISHERINV", 1, 1, Statistical, S, "FISHERINV(y)", "Inverse of the Fisher transformation.", |a: &[Arg], _c: C| num_val(num(a, 0)?.tanh())),
        f!("PHI", 1, 1, Statistical, S, "PHI(x)", "Standard normal density.", |a: &[Arg], _c: C| num_val(norm_pdf(num(a, 0)?))),
        f!("GAUSS", 1, 1, Statistical, S, "GAUSS(z)", "Probability that a standard normal value falls between the mean and z.", gauss),
    ]
}

#[cfg(test)]
mod tests;
