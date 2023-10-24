mod data_structures;
mod utils;
pub use data_structures::*;

#[cfg(test)]
mod tests;

use ark_crypto_primitives::sponge::poseidon::PoseidonSponge;
use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use ark_ff::PrimeField;
use ark_poly::MultilinearExtension;
use ark_std::{rand::RngCore, string::ToString, vec::Vec, UniformRand};
use blake2::Blake2s256;
use core::marker::PhantomData;
use digest::Digest;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use crate::hyrax::utils::tensor_prime;
use crate::utils::{inner_product, scalar_by_vector, vector_sum, IOPTranscript, Matrix};

use crate::{
    challenge::ChallengeGenerator, hyrax::utils::flat_to_matrix_column_major, Error,
    LabeledCommitment, LabeledPolynomial, PolynomialCommitment,
};

/// String of bytes used to seed the randomness during the setup function.
/// Note that the latter should never be used in production environments.
pub const PROTOCOL_NAME: &'static [u8] = b"Hyrax protocol";

/// Hyrax polynomial committment scheme:
/// A polynomial commitment scheme based on the hardness of the
/// discrete logarithm problem in prime-order groups. This is a
/// Fiat-Shamired version of the PCS described in the Hyrax paper
/// [[WTsTW17]][hyrax].
///
/// [hyrax]: https://eprint.iacr.org/2017/1132.pdf
///
/// * Modification note *
///
/// In the PCS contained in the cited article, the verifier never learns the
/// actual evaluation of the polynomial at the requested point, but is instead
/// convinced that a previously received Pedersen commitment is indeed a
/// commitment to said evaluation - this is what the SNARK proposed therein
/// necessitates. However, the Arkworks framework requies the verifier to
/// actually learn that value, which is why we have added the opening of
/// the commitment at the end of the protocol. This likely does not result in
/// an optimal non-hiding PCS, but we feel it is the most faithful adaptation
/// of the original PCS that can be implemented with the current restrictions.
pub struct HyraxPC<
    // The elliptic curve used for Pedersen commitments (only EC groups are
    // supported as of now).
    G: AffineRepr,
    // A polynomial type representing multilinear polynomials
    P: MultilinearExtension<G::ScalarField>,
> {
    _phantom: PhantomData<(G, P)>,
}

// TODO Outstanding issues
// - Will ark_std::cfg_iter! lead to any performance gains?
//   currently only used in the performance bottleneck: multiexponentiation
// - Do any of the following methods have a natural implementation which is
//   more efficient than the default?
//   batch_check, batch_open, open_combinations check_combinations
//   The reference article does not mention this
// - Is it safe to open several polynomials at once, and if so, is there a
//   more efficient way than what is done below? (which simply shares the
//   computation of L and R across all polynomials)
// - Implement optimisation from section `Reducing the cost of
//   proof-of-dot-prod` in the reference article.

impl<G: AffineRepr, P: MultilinearExtension<G::ScalarField>> HyraxPC<G, P> {
    /// Pedersen commitment to a vector of scalars as described in appendix A.1
    /// of the reference article.
    /// The caller must either directly pass hiding exponent `r` inside Some,
    /// or provide an rng so that `r` can be sampled.
    /// If there are `n` scalars, the first `n` elements of the key will be
    /// multiplied by them in the same order, and its `n + 1`th element will be
    /// multiplied by `r`.
    ///
    /// # Panics
    ///
    /// Panics if both `r` and `rng` are None.
    fn pedersen_commit(
        key: &HyraxCommitterKey<G>,
        scalars: &[G::ScalarField],
        r: Option<G::ScalarField>,
        rng: Option<&mut dyn RngCore>,
    ) -> (G, G::ScalarField) {
        // Cannot use unwrap_or, since its argument is always evaluated
        let r = match r {
            Some(v) => v,
            None => G::ScalarField::rand(rng.expect("Either r or rng must be provided")),
        };

        let mut scalars_ext = Vec::from(scalars);
        scalars_ext.push(r);

        // Trimming the key to the length of the coefficient vector
        let mut points_ext = key.com_key[0..scalars.len()].to_vec();
        points_ext.push(key.h);

        let scalars_bigint = scalars_ext
            .iter()
            .map(|s| s.into_bigint())
            .collect::<Vec<_>>();

        // Multi-exponentiation in the group of points of the EC
        let com = <G::Group as VariableBaseMSM>::msm_bigint(&points_ext, &scalars_bigint);

        (com.into(), r)
    }
}

impl<G: AffineRepr, P: MultilinearExtension<G::ScalarField>>
    PolynomialCommitment<
        G::ScalarField,
        P,
        // Dummy sponge - required by the trait, not used in this implementation
        PoseidonSponge<G::ScalarField>,
    > for HyraxPC<G, P>
{
    type UniversalParams = HyraxUniversalParams<G>;
    type CommitterKey = HyraxCommitterKey<G>;
    type VerifierKey = HyraxVerifierKey<G>;
    type PreparedVerifierKey = HyraxPreparedVerifierKey<G>;
    type Commitment = HyraxCommitment<G>;
    type PreparedCommitment = HyraxPreparedCommitment<G>;
    type Randomness = HyraxRandomness<G::ScalarField>;
    type Proof = Vec<HyraxProof<G>>;
    type BatchProof = Vec<Self::Proof>;
    type Error = Error;

    /// Outputs mock universal parameters for the Hyrax polynomial commitment
    /// scheme. It does *not* return random keys across calls and should never
    /// be used in settings where security is required - it is only useful for
    /// testing.
    ///
    /// # Panics
    ///
    /// Panics if `num_vars` is None or contains an odd value.
    fn setup<R: RngCore>(
        _max_degree: usize,
        num_vars: Option<usize>,
        _rng: &mut R,
    ) -> Result<Self::UniversalParams, Self::Error> {
        let n = num_vars.expect("Hyrax requires num_vars to be specified");

        assert_eq!(
            n % 2,
            0,
            "Only polynomials with an even number of variables \
                    are supported in this implementation"
        );

        // Number of rows (or, equivalently, colums) of a square matrix
        // containing the coefficients of an n-variate ML polynomial
        let dim = 1 << n / 2;

        // The following block of code is largely taking from the IPA module
        // in this crate. It generates random points (not guaranteed to be
        // generators, since the point at infinity should theoretically occur)
        let points: Vec<_> = ark_std::cfg_into_iter!(0u64..dim + 1)
            .map(|i| {
                let mut hash =
                    Blake2s256::digest([PROTOCOL_NAME, &i.to_le_bytes()].concat().as_slice());
                let mut p = G::from_random_bytes(&hash);
                let mut j = 0u64;
                while p.is_none() {
                    let mut bytes = PROTOCOL_NAME.to_vec();
                    bytes.extend(i.to_le_bytes());
                    bytes.extend(j.to_le_bytes());
                    hash = Blake2s256::digest(bytes.as_slice());
                    p = G::from_random_bytes(&hash);
                    j += 1;
                }
                let point = p.unwrap();
                point.mul_by_cofactor_to_group()
            })
            .collect();

        // Converting from projective to affine representation
        let mut points = G::Group::normalize_batch(&points);

        let h: G = points.pop().unwrap();

        Ok(HyraxUniversalParams { com_key: points, h })
    }

    /// Trims a key into a prover key and a verifier key. This should only
    /// amount to discarding some of the points in said key if the prover
    /// and verifier only wish to commit to polynomials with fewer variables
    /// than the key can support. Since the number of variables is not
    /// considered in the prototype, this function currently simply clones the
    /// key.
    fn trim(
        pp: &Self::UniversalParams,
        _supported_degree: usize,
        _supported_hiding_bound: usize,
        _enforced_degree_bounds: Option<&[usize]>,
    ) -> Result<(Self::CommitterKey, Self::VerifierKey), Self::Error> {
        Ok((pp.clone(), pp.clone()))
    }

    /// Produces a list of commitments to the passed polynomials. Cf. the
    /// section "Square-root commitment scheme" from the reference article.
    ///
    /// # Panics
    ///
    /// Panics if `rng` is None, since Hyrax requires randomness in order to
    /// commit to a polynomial
    fn commit<'a>(
        ck: &Self::CommitterKey,
        polynomials: impl IntoIterator<Item = &'a LabeledPolynomial<G::ScalarField, P>>,
        rng: Option<&mut dyn RngCore>,
    ) -> Result<
        (
            Vec<LabeledCommitment<Self::Commitment>>,
            Vec<Self::Randomness>,
        ),
        Self::Error,
    >
    where
        P: 'a,
    {
        let mut coms = Vec::new();
        let mut rands = Vec::new();

        let rng_inner = rng.expect("Committing to polynomials requires a random generator");

        for l_poly in polynomials {
            let mut com_rands = Vec::new();

            let label = l_poly.label();
            let poly = l_poly.polynomial();

            let n = poly.num_vars();
            let dim = 1 << n / 2;

            assert_eq!(
                n % 2,
                0,
                "Only points with an even number of variables \
                are supported in this implementation"
            );

            assert!(
                n <= ck.com_key.len(),
                "Attempted to commit to a polynomial with {n} variables, but
                this key only supports up to {} variables",
                ck.com_key.len()
            );

            let m = flat_to_matrix_column_major(&poly.to_evaluations(), dim, dim);

            // Commiting to the matrix with one multi-commitment per row
            let row_coms = m
                .iter()
                .map(|row| {
                    let (c, r) = Self::pedersen_commit(ck, &row, None, Some(rng_inner));
                    // Storing the randomness used in the commitment
                    com_rands.push(r);
                    c
                })
                .collect();

            let com = HyraxCommitment { row_coms };
            let l_comm = LabeledCommitment::new(label.to_string(), com, Some(1));

            coms.push(l_comm);
            rands.push(com_rands);
        }

        Ok((coms, rands))
    }

    /// Opens a list of polynomial commitments at a desired point. This
    /// requires the list of original polynomials (`labeled_polynomials`) as
    /// well as the random values using by the Pedersen multi-commits during
    /// the commitment phase (`randomness`). Cf. sections "Square-root
    /// commitment scheme" and appendix A.2 from the reference article.
    ///
    /// # Panics
    ///
    /// Panics if
    /// - `rng` is None, since Hyrax requires randomness in order to
    /// open the commitment to a polynomial.
    /// - The point doesn't have an even number of variables.
    /// - The labels of a commitment doesn't match that of the corresponding
    /// polynomial.
    /// - The number of variables of a polynomial doesn't match that of the
    /// point.
    ///
    /// # Disregarded arguments
    /// - `opening_challenges`
    fn open<'a>(
        ck: &Self::CommitterKey,
        labeled_polynomials: impl IntoIterator<Item = &'a LabeledPolynomial<G::ScalarField, P>>,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        point: &'a P::Point,
        // Not used and not generic on the cryptographic sponge S
        _opening_challenges: &mut ChallengeGenerator<
            G::ScalarField,
            PoseidonSponge<G::ScalarField>,
        >,
        rands: impl IntoIterator<Item = &'a Self::Randomness>,
        rng: Option<&mut dyn RngCore>,
    ) -> Result<Self::Proof, Self::Error>
    where
        Self::Commitment: 'a,
        Self::Randomness: 'a,
        P: 'a,
    {
        let n = point.len();

        assert_eq!(
            n % 2,
            0,
            "Only points with an even number of variables \
            are supported in this implementation"
        );

        let dim = 1 << n / 2;

        // Reversing the point is necessary because the MLE interface returns
        // evaluations in little-endian order
        let point_rev: Vec<G::ScalarField> = point.iter().rev().cloned().collect();

        let point_lower = &point_rev[n / 2..];
        let point_upper = &point_rev[..n / 2];

        // Deriving the tensors which result in the evaluation of the polynomial
        // when they are multiplied by the coefficient matrix.
        let l = tensor_prime(point_lower);
        let r = tensor_prime(point_upper);

        let mut proofs = Vec::new();

        let rng_inner = rng.expect("Opening polynomials requires randomness");

        for (l_poly, (l_com, randomness)) in labeled_polynomials
            .into_iter()
            .zip(commitments.into_iter().zip(rands.into_iter()))
        {
            let label = l_poly.label();
            assert_eq!(
                label,
                l_com.label(),
                "Mismatching labels: {label} and {}",
                l_com.label()
            );

            let poly = l_poly.polynomial();
            let com = l_com.commitment();

            assert_eq!(
                poly.num_vars(),
                n,
                "The committed polynomial has {} variables, but \
                the point has {n} variables",
                poly.num_vars()
            );

            // Initialising the transcript
            let mut transcript: IOPTranscript<G::ScalarField> = IOPTranscript::new(b"transcript");

            // Absorbing public parameters
            transcript.append_serializable_element(b"public parameters", ck)?;

            // Absorbing the commitment to the polynomial
            transcript.append_serializable_element(b"commitment", &com.row_coms)?;

            // Absorbing the point
            transcript.append_serializable_element(b"point", point)?;

            // Commiting to the matrix formed by the polynomial coefficients
            let t_aux = flat_to_matrix_column_major(&poly.to_evaluations(), dim, dim);
            let t = Matrix::new_from_rows(t_aux);

            let lt = t.row_mul(&l);

            // t_prime coincides witht he Pedersen commitment to lt with the
            // randomnes r_lt computed here
            let r_lt = l
                .iter()
                .zip(randomness.iter())
                .map(|(l, r)| *l * r)
                .sum::<G::ScalarField>();

            let eval = inner_product(&lt, &r);

            // Singleton commit
            let (com_eval, r_eval) = Self::pedersen_commit(ck, &[eval], None, Some(rng_inner));

            // ******** Dot product argument ********
            // Appendix A.2 in the reference article

            let d: Vec<G::ScalarField> =
                (0..dim).map(|_| G::ScalarField::rand(rng_inner)).collect();

            let b = inner_product(&r, &d);

            // Multi-commit
            let (com_d, r_d) = Self::pedersen_commit(ck, &d, None, Some(rng_inner));

            // Singleton commit
            let (com_b, r_b) = Self::pedersen_commit(ck, &[b], None, Some(rng_inner));

            // Absorbing the commitment to the evaluation
            transcript.append_serializable_element(b"com_eval", &com_eval)?;

            // Absorbing the two auxiliary commitments
            transcript.append_serializable_element(b"com_d", &com_d)?;
            transcript.append_serializable_element(b"com_b", &com_b)?;

            // Receive the random challenge c from the verifier, i.e. squeeze
            // it from the transcript.
            let c = transcript.get_and_append_challenge(b"c").unwrap();

            let z = vector_sum(&d, &scalar_by_vector(c, &lt));
            let z_d = c * r_lt + r_d;
            let z_b = c * r_eval + r_b;

            // ******** Opening ********
            // This is *not* part of the Hyrax PCS as described in the reference
            // article. Cf. the "Modification note" at the beginning of this file.
            // From the prover's perspective, opening amounts to adding r_eval to
            // the proof.

            proofs.push(HyraxProof {
                com_eval,
                com_d,
                com_b,
                z,
                z_d,
                z_b,
                r_eval,
            });
        }

        Ok(proofs)
    }

    /// Verifies a list of opening proofs and confirms the evaluation of the
    /// committed polynomials at the desired point.
    ///
    /// # Panics
    /// - If the point doesn't have an even number of variables.
    /// - If the length of a commitment does not correspond to the length of the
    /// point (specifically, commitment length should be 2^(point-length/2)).
    ///
    /// # Disregarded arguments
    /// - `opening_challenges`
    /// - `rng`
    fn check<'a>(
        vk: &Self::VerifierKey,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        point: &'a P::Point,
        values: impl IntoIterator<Item = G::ScalarField>,
        proof: &Self::Proof,
        // Not used and not generic on the cryptographic sponge S
        _opening_challenges: &mut ChallengeGenerator<
            G::ScalarField,
            PoseidonSponge<G::ScalarField>,
        >,
        _rng: Option<&mut dyn RngCore>,
    ) -> Result<bool, Self::Error>
    where
        Self::Commitment: 'a,
    {
        let n = point.len();

        assert_eq!(
            n % 2,
            0,
            "Only points with an even number of variables \
            are supported in this implementation"
        );

        // Reversing the point is necessary because the MLE interface returns
        // evaluations in little-endian order
        let point_rev: Vec<G::ScalarField> = point.iter().rev().cloned().collect();

        let point_lower = &point_rev[n / 2..];
        let point_upper = &point_rev[..n / 2];

        // Deriving the tensors which result in the evaluation of the polynomial
        // when they are multiplied by the coefficient matrix.
        let l = tensor_prime(point_lower);
        let r = tensor_prime(point_upper);

        for (com, (claim, h_proof)) in commitments
            .into_iter()
            .zip(values.into_iter().zip(proof.into_iter()))
        {
            let row_coms = &com.commitment().row_coms;

            // extract each field from h_proof
            let HyraxProof {
                com_eval,
                com_d,
                com_b,
                z,
                z_d,
                z_b,
                r_eval,
            } = h_proof;

            assert_eq!(
                row_coms.len(),
                1 << n / 2,
                "The commitment should have 2^(n/2) = has {} entries, but \
                it has {} instead",
                1 << n / 2,
                row_coms.len()
            );

            // Computing t_prime with a multi-exponentiation
            let l_bigint = l.iter().map(|chi| chi.into_bigint()).collect::<Vec<_>>();
            let t_prime: G = <G::Group as VariableBaseMSM>::msm_bigint(&row_coms, &l_bigint).into();

            // Construct transcript and squeeze the challenge c from it

            let mut transcript: IOPTranscript<G::ScalarField> = IOPTranscript::new(b"transcript");

            // Absorbing public parameters
            transcript.append_serializable_element(b"public parameters", vk)?;

            // Absorbing the commitment to the polynomial
            transcript.append_serializable_element(b"commitment", row_coms)?;

            // Absorbing the point
            transcript.append_serializable_element(b"point", point)?;

            // Absorbing the commitment to the evaluation
            transcript.append_serializable_element(b"com_eval", com_eval)?;

            // Absorbing the two auxiliary commitments
            transcript.append_serializable_element(b"com_d", com_d)?;
            transcript.append_serializable_element(b"com_b", com_b)?;

            // Receive the random challenge c from the verifier, i.e. squeeze
            // it from the transcript.
            let c = transcript.get_and_append_challenge(b"c").unwrap();

            // First check
            let com_z_zd = Self::pedersen_commit(vk, &z, Some(*z_d), None).0;
            if com_z_zd != (t_prime.mul(c) + com_d).into() {
                return Ok(false);
            }

            // Second check
            let com_dp = Self::pedersen_commit(vk, &[inner_product(&r, &z)], Some(*z_b), None).0;
            if com_dp != (com_eval.mul(c) + com_b).into() {
                return Ok(false);
            }

            // Third check: opening
            let exp = Self::pedersen_commit(vk, &[claim], Some(*r_eval), None).0;

            if *com_eval != exp {
                return Ok(false);
            }
        }

        Ok(true)
    }
}
