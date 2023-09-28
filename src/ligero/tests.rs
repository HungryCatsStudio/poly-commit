#[cfg(test)]
mod tests {

    use crate::ark_std::UniformRand;
    use crate::{
        challenge::ChallengeGenerator,
        ligero::{utils::*, Ligero, LigeroPCUniversalParams, PolynomialCommitment},
        LabeledPolynomial,
    };
    use ark_bls12_377::Fr;
    use ark_bls12_377::{Bls12_377, Fq};
    use ark_bls12_381::Bls12_381;
    use ark_bls12_381::Fr as Fr381;
    use ark_crypto_primitives::sponge::poseidon::PoseidonConfig;
    use ark_crypto_primitives::sponge::CryptographicSponge;
    use ark_crypto_primitives::{
        crh::{pedersen, sha256::Sha256, CRHScheme, TwoToOneCRHScheme},
        merkle_tree::{ByteDigestConverter, Config},
        sponge::poseidon::PoseidonSponge,
    };
    use ark_ec::pairing::Pairing;
    use ark_ff::PrimeField;
    use ark_poly::{
        domain::general::GeneralEvaluationDomain, univariate::DensePolynomial, DenseUVPolynomial,
        EvaluationDomain, Polynomial,
    };
    use ark_std::test_rng;
    use blake2::Blake2s256;
    use core::marker::PhantomData;
    use rand_chacha::{rand_core::SeedableRng, ChaCha20Rng};

    type UniPoly = DensePolynomial<Fr>;
    #[derive(Clone)]
    pub(super) struct Window4x256;
    impl pedersen::Window for Window4x256 {
        const WINDOW_SIZE: usize = 4;
        const NUM_WINDOWS: usize = 256;
    }

    type LeafH = Sha256; //::CRH<JubJub, Window4x256>;
    type CompressH = Sha256; //pedersen::TwoToOneCRH<JubJub, Window4x256>;

    struct MerkleTreeParams;

    impl Config for MerkleTreeParams {
        type Leaf = [u8];
        // type Leaf = Vec<u8>;

        type LeafDigest = <LeafH as CRHScheme>::Output;
        type LeafInnerDigestConverter = ByteDigestConverter<Self::LeafDigest>;
        type InnerDigest = <CompressH as TwoToOneCRHScheme>::Output;

        type LeafHash = LeafH;
        type TwoToOneHash = CompressH;
    }

    type MTConfig = MerkleTreeParams;
    type Sponge = PoseidonSponge<Fr>;
    type PC<F, C, D, S, P> = Ligero<F, C, D, S, P>;
    type LigeroPCS = PC<Fr, MTConfig, Blake2s256, Sponge, UniPoly>;
    type LigeroPcsF<F> = PC<F, MTConfig, Blake2s256, Sponge, DensePolynomial<F>>;

    #[test]
    fn test_matrix_constructor_flat() {
        let entries: Vec<Fr> = to_field(vec![10, 100, 4, 67, 44, 50]);
        let mat = Matrix::new_from_flat(2, 3, &entries);
        assert_eq!(mat.entry(1, 2), Fr::from(50));
    }

    #[test]
    fn test_matrix_constructor_flat_square() {
        let entries: Vec<Fr> = to_field(vec![10, 100, 4, 67]);
        let mat = Matrix::new_from_flat(2, 2, &entries);
        assert_eq!(mat.entry(1, 1), Fr::from(67));
    }

    #[test]
    #[should_panic(expected = "dimensions are 2 x 3 but entry vector has 5 entries")]
    fn test_matrix_constructor_flat_panic() {
        let entries: Vec<Fr> = to_field(vec![10, 100, 4, 67, 44]);
        Matrix::new_from_flat(2, 3, &entries);
    }

    #[test]
    fn test_matrix_constructor_rows() {
        let rows: Vec<Vec<Fr>> = vec![
            to_field(vec![10, 100, 4]),
            to_field(vec![23, 1, 0]),
            to_field(vec![55, 58, 9]),
        ];
        let mat = Matrix::new_from_rows(rows);
        assert_eq!(mat.entry(2, 0), Fr::from(55));
    }

    #[test]
    #[should_panic(expected = "not all rows have the same length")]
    fn test_matrix_constructor_rows_panic() {
        let rows: Vec<Vec<Fr>> = vec![
            to_field(vec![10, 100, 4]),
            to_field(vec![23, 1, 0]),
            to_field(vec![55, 58]),
        ];
        Matrix::new_from_rows(rows);
    }

    #[test]
    fn test_cols() {
        let rows: Vec<Vec<Fr>> = vec![
            to_field(vec![4, 76]),
            to_field(vec![14, 92]),
            to_field(vec![17, 89]),
        ];

        let mat = Matrix::new_from_rows(rows);

        assert_eq!(mat.cols()[1], to_field(vec![76, 92, 89]));
    }

    #[test]
    fn test_row_mul() {
        let rows: Vec<Vec<Fr>> = vec![
            to_field(vec![10, 100, 4]),
            to_field(vec![23, 1, 0]),
            to_field(vec![55, 58, 9]),
        ];

        let mat = Matrix::new_from_rows(rows);
        let v: Vec<Fr> = to_field(vec![12, 41, 55]);
        // by giving the result in the integers and then converting to Fr
        // we ensure the test will still pass even if Fr changes
        assert_eq!(mat.row_mul(&v), to_field::<Fr>(vec![4088, 4431, 543]));
    }

    #[test]
    fn test_encoding() {
        // we use this polynomial to generate the the values we will ask the fft to interpolate

        let rho_inv = 3;
        // `i` is the min number of evaluations we need to interpolate a poly of degree `i - 1`
        for i in 1..10 {
            let deg = (1 << i) - 1;

            let rand_chacha = &mut ChaCha20Rng::from_rng(test_rng()).unwrap();
            let mut pol = rand_poly::<Fr>(deg, None, rand_chacha);

            while pol.degree() != deg {
                pol = rand_poly::<Fr>(deg, None, rand_chacha);
            }

            let coeffs = &pol.coeffs;

            // size of evals might be larger than deg + 1 (the min. number of evals needed to interpolate): we could still do R-S encoding on smaller evals, but the resulting polynomial will differ, so for this test to work we should pass it in full
            let m = deg + 1;

            let encoded = reed_solomon(&coeffs, rho_inv);

            let large_domain = GeneralEvaluationDomain::<Fr>::new(m * rho_inv).unwrap();

            // the encoded elements should agree with the evaluations of the polynomial in the larger domain
            for j in 0..(rho_inv * m) {
                assert_eq!(pol.evaluate(&large_domain.element(j)), encoded[j]);
            }
        }
    }

    #[test]
    fn test_merkle_tree() {
        let mut rng = &mut test_rng();
        let leaf_hash_params = <LeafH as CRHScheme>::setup(&mut rng).unwrap();
        let two_to_one_params = <CompressH as TwoToOneCRHScheme>::setup(&mut rng)
            .unwrap()
            .clone();

        let rows: Vec<Vec<Fr>> = vec![
            to_field(vec![4, 76]),
            to_field(vec![14, 92]),
            to_field(vec![17, 89]),
        ];

        let mat = Matrix::new_from_rows(rows);
        let mt = LigeroPCS::create_merkle_tree(&mat, &leaf_hash_params, &two_to_one_params);

        let root = mt.root();

        for (i, col) in mat.cols().iter().enumerate() {
            let col_hash = hash_column::<Blake2s256, Fr>(col);

            let proof = mt.generate_proof(i).unwrap();
            assert!(proof
                .verify(
                    &leaf_hash_params,
                    &two_to_one_params,
                    &root,
                    col_hash.clone()
                )
                .unwrap());
        }
    }

    #[test]
    fn test_get_num_bytes() {
        assert_eq!(get_num_bytes(0), 0);
        assert_eq!(get_num_bytes(1), 1);
        assert_eq!(get_num_bytes(9), 1);
        assert_eq!(get_num_bytes(1 << 11), 2);
        assert_eq!(get_num_bytes(1 << 32 - 1), 4);
        assert_eq!(get_num_bytes(1 << 32), 5);
        assert_eq!(get_num_bytes(1 << 32 + 1), 5);
    }

    fn rand_poly<Fr: PrimeField>(
        degree: usize,
        _: Option<usize>,
        rng: &mut ChaCha20Rng,
    ) -> DensePolynomial<Fr> {
        DensePolynomial::rand(degree, rng)
    }

    fn constant_poly<Fr: PrimeField>(
        _: usize,
        _: Option<usize>,
        rng: &mut ChaCha20Rng,
    ) -> DensePolynomial<Fr> {
        DensePolynomial::from_coefficients_slice(&[Fr::rand(rng)])
    }

    // TODO: replace by https://github.com/arkworks-rs/crypto-primitives/issues/112.
    fn test_sponge<F: PrimeField>() -> PoseidonSponge<F> {
        let full_rounds = 8;
        let partial_rounds = 31;
        let alpha = 17;

        let mds = vec![
            vec![F::one(), F::zero(), F::one()],
            vec![F::one(), F::one(), F::zero()],
            vec![F::zero(), F::one(), F::one()],
        ];

        let mut v = Vec::new();
        let mut ark_rng = test_rng();

        for _ in 0..(full_rounds + partial_rounds) {
            let mut res = Vec::new();

            for _ in 0..3 {
                res.push(F::rand(&mut ark_rng));
            }
            v.push(res);
        }
        let config = PoseidonConfig::new(full_rounds, partial_rounds, alpha, mds, v, 2, 1);
        PoseidonSponge::new(&config)
    }

    #[test]
    fn test_setup() {
        let rng = &mut test_rng();
        let _ = LigeroPcsF::<Fq>::setup(1 << 44, None, rng).unwrap();
        // but the base field of bls12_381 doesnt have such large domains
        use ark_bls12_381::Fq as F_381;
        assert_eq!(LigeroPcsF::<F_381>::setup(20, None, rng).is_err(), true);
    }

    #[test]
    fn test_construction() {
        let degree = 4;
        let mut rng = &mut test_rng();
        // just to make sure we have the right degree given the FFT domain for our field
        LigeroPCS::setup(degree, None, rng).unwrap();
        let leaf_hash_params = <LeafH as CRHScheme>::setup(&mut rng).unwrap();
        let two_to_one_params = <CompressH as TwoToOneCRHScheme>::setup(&mut rng)
            .unwrap()
            .clone();
        let check_well_formedness = true;

        let pp: LigeroPCUniversalParams<Fr, MTConfig> = LigeroPCUniversalParams {
            _field: PhantomData,
            sec_param: 128,
            rho_inv: 4,
            check_well_formedness,
            leaf_hash_params,
            two_to_one_params,
        };

        let (ck, vk) = LigeroPCS::trim(&pp, 0, 0, None).unwrap();

        let rand_chacha = &mut ChaCha20Rng::from_rng(test_rng()).unwrap();
        let labeled_poly = LabeledPolynomial::new(
            "test".to_string(),
            rand_poly(degree, None, rand_chacha),
            None,
            None,
        );

        let mut test_sponge = test_sponge::<Fr>();
        let (c, rands) = LigeroPCS::commit(&ck, &[labeled_poly.clone()], None).unwrap();

        let point = Fr::rand(rand_chacha);

        let value = labeled_poly.evaluate(&point);

        let mut challenge_generator: ChallengeGenerator<Fr, PoseidonSponge<Fr>> =
            ChallengeGenerator::new_univariate(&mut test_sponge);

        // assert!(
        //     LigeroPCS::check_well_formedness(
        //         &c[0].commitment(),
        //         &leaf_hash_params,
        //         &two_to_one_params
        //     )
        //     .is_ok(),
        //     "Well formedness check failed"
        // );

        let proof = LigeroPCS::open(
            &ck,
            &[labeled_poly],
            &c,
            &point,
            &mut (challenge_generator.clone()),
            &rands,
            None,
        )
        .unwrap();
        assert!(LigeroPCS::check(
            &vk,
            &c,
            &point,
            [value],
            &proof,
            &mut challenge_generator,
            None
        )
        .unwrap());
    }

    #[test]
    fn test_several_polynomials() {
        let degrees = [4_usize, 13_usize, 30_usize];
        let mut rng = &mut test_rng();

        LigeroPCS::setup(*degrees.iter().max().unwrap(), None, rng).unwrap();
        let leaf_hash_params = <LeafH as CRHScheme>::setup(&mut rng).unwrap();
        let two_to_one_params = <CompressH as TwoToOneCRHScheme>::setup(&mut rng)
            .unwrap()
            .clone();
        let check_well_formedness = true;

        let pp: LigeroPCUniversalParams<Fr, MTConfig> = LigeroPCUniversalParams {
            _field: PhantomData,
            sec_param: 128,
            rho_inv: 4,
            check_well_formedness,
            leaf_hash_params,
            two_to_one_params,
        };

        let (ck, vk) = LigeroPCS::trim(&pp, 0, 0, None).unwrap();

        let rand_chacha = &mut ChaCha20Rng::from_rng(test_rng()).unwrap();
        let mut test_sponge = test_sponge::<Fr>();
        let mut challenge_generator: ChallengeGenerator<Fr, PoseidonSponge<Fr>> =
            ChallengeGenerator::new_univariate(&mut test_sponge);

        let mut labeled_polys = Vec::new();
        let mut values = Vec::new();

        let point = Fr::rand(rand_chacha);

        for degree in degrees {
            let labeled_poly = LabeledPolynomial::new(
                "test".to_string(),
                rand_poly(degree, None, rand_chacha),
                None,
                None,
            );

            values.push(labeled_poly.evaluate(&point));
            labeled_polys.push(labeled_poly);
        }

        let (commitments, randomness) = LigeroPCS::commit(&ck, &labeled_polys, None).unwrap();

        let proof = LigeroPCS::open(
            &ck,
            &labeled_polys,
            &commitments,
            &point,
            &mut (challenge_generator.clone()),
            &randomness,
            None,
        )
        .unwrap();
        assert!(LigeroPCS::check(
            &vk,
            &commitments,
            &point,
            values,
            &proof,
            &mut challenge_generator,
            None
        )
        .unwrap());
    }

    #[test]
    #[should_panic(expected = "Mismatched lengths")]
    fn test_several_polynomials_mismatched_lengths() {
        // here we go through the same motions as in test_several_polynomials,
        // but pass to check() one fewer value than we should
        let degrees = [4_usize, 13_usize, 30_usize];
        let mut rng = &mut test_rng();

        LigeroPCS::setup(*degrees.iter().max().unwrap(), None, rng).unwrap();
        let leaf_hash_params = <LeafH as CRHScheme>::setup(&mut rng).unwrap();
        let two_to_one_params = <CompressH as TwoToOneCRHScheme>::setup(&mut rng)
            .unwrap()
            .clone();

        let check_well_formedness = true;

        let pp: LigeroPCUniversalParams<Fr, MTConfig> = LigeroPCUniversalParams {
            _field: PhantomData,
            sec_param: 128,
            rho_inv: 4,
            check_well_formedness,
            leaf_hash_params,
            two_to_one_params,
        };

        let (ck, vk) = LigeroPCS::trim(&pp, 0, 0, None).unwrap();

        let rand_chacha = &mut ChaCha20Rng::from_rng(test_rng()).unwrap();
        let mut test_sponge = test_sponge::<Fr>();
        let mut challenge_generator: ChallengeGenerator<Fr, PoseidonSponge<Fr>> =
            ChallengeGenerator::new_univariate(&mut test_sponge);

        let mut labeled_polys = Vec::new();
        let mut values = Vec::new();

        let point = Fr::rand(rand_chacha);

        for degree in degrees {
            let labeled_poly = LabeledPolynomial::new(
                "test".to_string(),
                rand_poly(degree, None, rand_chacha),
                None,
                None,
            );

            values.push(labeled_poly.evaluate(&point));
            labeled_polys.push(labeled_poly);
        }

        let (commitments, randomness) = LigeroPCS::commit(&ck, &labeled_polys, None).unwrap();

        let proof = LigeroPCS::open(
            &ck,
            &labeled_polys,
            &commitments,
            &point,
            &mut (challenge_generator.clone()),
            &randomness,
            None,
        )
        .unwrap();
        assert!(LigeroPCS::check(
            &vk,
            &commitments,
            &point,
            values[0..2].to_vec(),
            &proof,
            &mut challenge_generator,
            None
        )
        .unwrap());
    }

    #[test]
    #[should_panic]
    fn test_several_polynomials_swap_proofs() {
        // in this test we work with three polynomials and swap the proofs of the first and last openings
        let degrees = [4_usize, 13_usize, 30_usize];
        let mut rng = &mut test_rng();

        LigeroPCS::setup(*degrees.iter().max().unwrap(), None, rng).unwrap();
        let leaf_hash_params = <LeafH as CRHScheme>::setup(&mut rng).unwrap();
        let two_to_one_params = <CompressH as TwoToOneCRHScheme>::setup(&mut rng)
            .unwrap()
            .clone();
        let check_well_formedness = true;

        let pp: LigeroPCUniversalParams<Fr, MTConfig> = LigeroPCUniversalParams {
            _field: PhantomData,
            sec_param: 128,
            rho_inv: 4,
            check_well_formedness,
            leaf_hash_params,
            two_to_one_params,
        };

        let (ck, vk) = LigeroPCS::trim(&pp, 0, 0, None).unwrap();

        let rand_chacha = &mut ChaCha20Rng::from_rng(test_rng()).unwrap();
        let mut test_sponge = test_sponge::<Fr>();
        let mut challenge_generator: ChallengeGenerator<Fr, PoseidonSponge<Fr>> =
            ChallengeGenerator::new_univariate(&mut test_sponge);

        let mut labeled_polys = Vec::new();
        let mut values = Vec::new();

        let point = Fr::rand(rand_chacha);

        for degree in degrees {
            let labeled_poly = LabeledPolynomial::new(
                "test".to_string(),
                rand_poly(degree, None, rand_chacha),
                None,
                None,
            );

            values.push(labeled_poly.evaluate(&point));
            labeled_polys.push(labeled_poly);
        }

        let (commitments, randomness) = LigeroPCS::commit(&ck, &labeled_polys, None).unwrap();

        let mut proof = LigeroPCS::open(
            &ck,
            &labeled_polys,
            &commitments,
            &point,
            &mut (challenge_generator.clone()),
            &randomness,
            None,
        )
        .unwrap();

        // to do swap opening proofs
        proof.swap(0, 2);

        assert!(LigeroPCS::check(
            &vk,
            &commitments,
            &point,
            values,
            &proof,
            &mut challenge_generator,
            None
        )
        .unwrap());
    }

    #[test]
    #[should_panic]
    fn test_several_polynomials_swap_values() {
        // in this test we work with three polynomials and swap the second
        // and third values passed to the verifier externally
        let degrees = [4_usize, 13_usize, 30_usize];
        let mut rng = &mut test_rng();

        LigeroPCS::setup(*degrees.iter().max().unwrap(), None, rng).unwrap();
        let leaf_hash_params = <LeafH as CRHScheme>::setup(&mut rng).unwrap();
        let two_to_one_params = <CompressH as TwoToOneCRHScheme>::setup(&mut rng)
            .unwrap()
            .clone();
        let check_well_formedness = true;

        let pp: LigeroPCUniversalParams<Fr, MTConfig> = LigeroPCUniversalParams {
            _field: PhantomData,
            sec_param: 128,
            rho_inv: 4,
            check_well_formedness,
            leaf_hash_params,
            two_to_one_params,
        };

        let (ck, vk) = LigeroPCS::trim(&pp, 0, 0, None).unwrap();

        let rand_chacha = &mut ChaCha20Rng::from_rng(test_rng()).unwrap();
        let mut test_sponge = test_sponge::<Fr>();
        let mut challenge_generator: ChallengeGenerator<Fr, PoseidonSponge<Fr>> =
            ChallengeGenerator::new_univariate(&mut test_sponge);

        let mut labeled_polys = Vec::new();
        let mut values = Vec::new();

        let point = Fr::rand(rand_chacha);

        for degree in degrees {
            let labeled_poly = LabeledPolynomial::new(
                "test".to_string(),
                rand_poly(degree, None, rand_chacha),
                None,
                None,
            );

            values.push(labeled_poly.evaluate(&point));
            labeled_polys.push(labeled_poly);
        }

        let (commitments, randomness) = LigeroPCS::commit(&ck, &labeled_polys, None).unwrap();

        let proof = LigeroPCS::open(
            &ck,
            &labeled_polys,
            &commitments,
            &point,
            &mut (challenge_generator.clone()),
            &randomness,
            None,
        )
        .unwrap();

        // swap values externally passed to verifier
        values.swap(1, 2);

        assert!(LigeroPCS::check(
            &vk,
            &commitments,
            &point,
            values,
            &proof,
            &mut challenge_generator,
            None
        )
        .unwrap());
    }

    #[test]
    fn test_calculate_t_with_good_parameters() {
        assert!(calculate_t::<Fq>(128, 4, 2_usize.pow(32)).unwrap() < 200);
        assert!(calculate_t::<Fq>(256, 4, 2_usize.pow(32)).unwrap() < 400);
    }

    #[test]
    fn test_calculate_t_with_bad_parameters() {
        calculate_t::<Fq>((Fq::MODULUS_BIT_SIZE - 60) as usize, 4, 2_usize.pow(60)).unwrap_err();
        calculate_t::<Fq>(400, 4, 2_usize.pow(32)).unwrap_err();
    }

    fn rand_point<E: Pairing>(_: Option<usize>, rng: &mut ChaCha20Rng) -> E::ScalarField {
        E::ScalarField::rand(rng)
    }

    #[test]
    fn single_poly_test() {
        use crate::tests::*;
        single_poly_test::<_, _, LigeroPCS, _>(
            None,
            rand_poly::<Fr>,
            rand_point::<Bls12_377>,
            poseidon_sponge_for_test,
        )
        .expect("test failed for bls12-377");
        single_poly_test::<_, _, LigeroPcsF<Fr381>, _>(
            None,
            rand_poly::<Fr381>,
            rand_point::<Bls12_381>,
            poseidon_sponge_for_test,
        )
        .expect("test failed for bls12-381");
    }

    #[test]
    fn constant_poly_test() {
        use crate::tests::*;
        single_poly_test::<_, _, LigeroPCS, _>(
            None,
            constant_poly::<Fr>,
            rand_point::<Bls12_377>,
            poseidon_sponge_for_test,
        )
        .expect("test failed for bls12-377");
        single_poly_test::<_, _, LigeroPcsF<Fr381>, _>(
            None,
            constant_poly::<Fr381>,
            rand_point::<Bls12_381>,
            poseidon_sponge_for_test,
        )
        .expect("test failed for bls12-381");
    }
}
