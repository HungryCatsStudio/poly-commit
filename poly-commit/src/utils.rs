use ark_ff::Field;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::vec::Vec;

#[cfg(feature = "parallel")]
use rayon::{
    iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator},
    prelude::IndexedParallelIterator,
};

/// Return ceil(x / y).
pub(crate) fn ceil_div(x: usize, y: usize) -> usize {
    // XXX. warning: this expression can overflow.
    (x + y - 1) / y
}

#[derive(Derivative, CanonicalSerialize, CanonicalDeserialize)]
#[derivative(Default(bound = ""), Clone(bound = ""), Debug(bound = ""))]
pub(crate) struct Matrix<F: Field> {
    pub(crate) n: usize,
    pub(crate) m: usize,
    entries: Vec<Vec<F>>,
}

impl<F: Field> Matrix<F> {
    /// Returns a Matrix given a list of its rows, each in turn represented as a list of field elements.
    ///
    /// # Panics
    /// Panics if the sub-lists do not all have the same length.
    pub(crate) fn new_from_rows(row_list: Vec<Vec<F>>) -> Self {
        let m = row_list[0].len();

        for row in row_list.iter().skip(1) {
            assert_eq!(
                row.len(),
                m,
                "Invalid matrix construction: not all rows have the same length"
            );
        }

        Self {
            n: row_list.len(),
            m,
            entries: row_list,
        }
    }

    /// Returns the product v * self, where v is interpreted as a row vector. In other words,
    /// it returns a linear combination of the rows of self with coefficients given by v.
    ///
    /// Panics if the length of v is different from the number of rows of self.
    pub(crate) fn row_mul(&self, v: &[F]) -> Vec<F> {
        assert_eq!(
            v.len(),
            self.n,
            "Invalid row multiplication: vector has {} elements whereas each matrix column has {}",
            v.len(),
            self.n
        );

        cfg_into_iter!(0..self.m)
            .map(|col| {
                inner_product(
                    v,
                    &cfg_into_iter!(0..self.m)
                        .map(|row| self.entries[row][col])
                        .collect::<Vec<F>>(),
                )
            })
            .collect()
    }
}

#[inline]
pub(crate) fn inner_product<F: Field>(v1: &[F], v2: &[F]) -> F {
    ark_std::cfg_iter!(v1)
        .zip(v2)
        .map(|(li, ri)| *li * ri)
        .sum()
}

#[inline]
pub(crate) fn scalar_by_vector<F: Field>(s: F, v: &[F]) -> Vec<F> {
    ark_std::cfg_iter!(v).map(|x| *x * s).collect()
}

#[inline]
pub(crate) fn vector_sum<F: Field>(v1: &[F], v2: &[F]) -> Vec<F> {
    ark_std::cfg_iter!(v1)
        .zip(v2)
        .map(|(li, ri)| *li + ri)
        .collect()
}

// TODO: replace by https://github.com/arkworks-rs/crypto-primitives/issues/112.
#[cfg(test)]
use merlin::Transcript;
#[cfg(test)]
use ark_ff::PrimeField;

#[cfg(test)]
pub(crate) fn test_sponge<F: PrimeField>() -> Transcript {
    Transcript::new(b"domain")
}
