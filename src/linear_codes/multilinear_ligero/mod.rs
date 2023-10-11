use ark_crypto_primitives::{merkle_tree::Config, sponge::CryptographicSponge};
use ark_ff::PrimeField;
use ark_poly::{MultilinearExtension, Polynomial};
use ark_std::log2;
use ark_std::marker::PhantomData;
use ark_std::vec::Vec;

use super::utils::reed_solomon;
use super::LinearEncode;

mod tests;

/// The multilinear Ligero polynomial commitment scheme based on [[Ligero]][ligero].
/// The scheme defaults to the naive batching strategy.
///
/// Note: The scheme currently does not support hiding.
///
/// [ligero]: https://eprint.iacr.org/2022/1608.pdf
pub struct MultilinearLigero<
    F: PrimeField,
    C: Config,
    S: CryptographicSponge,
    P: MultilinearExtension<F>,
> {
    _phantom: PhantomData<(F, C, S, P)>,
}

impl<F, C, S, P> LinearEncode<F, P, C> for MultilinearLigero<F, C, S, P>
where
    F: PrimeField,
    C: Config,
    S: CryptographicSponge,
    P: MultilinearExtension<F>,
    P::Point: Into<Vec<F>>,
{
    fn encode(msg: &[F], rho_inv: usize) -> Vec<F> {
        reed_solomon(msg, rho_inv)
    }

    fn poly_repr(polynomial: &P) -> Vec<F> {
        polynomial.to_evaluations()
    }

    fn tensor(
        point: &<P as Polynomial<F>>::Point,
        left_len: usize,
        _right_len: usize,
    ) -> (Vec<F>, Vec<F>) {
        let point: Vec<F> = point.clone().into();

        let split = log2(left_len) as usize;
        let left = &point[..split];
        let right = &point[split..];
        (tensor_inner(left), tensor_inner(right))
    }
}

fn tensor_inner<F: PrimeField>(values: &[F]) -> Vec<F> {
    let one = F::one();
    let anti_values: Vec<F> = values.iter().map(|v| one - *v).collect();

    let mut layer: Vec<F> = vec![one];

    for i in 0..values.len() {
        let mut new_layer = Vec::new();
        for v in &layer {
            new_layer.push(*v * anti_values[i]);
        }
        for v in &layer {
            new_layer.push(*v * values[i]);
        }
        layer = new_layer;
    }

    layer
}
