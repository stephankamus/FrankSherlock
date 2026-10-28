//! The parts of face handling that are part of the index format.
//!
//! Not inference: `sherlock_app_lib::face` owns the SCRFD/ArcFace ONNX sessions. What
//! lives here is what the database stores and what any reader of `face_detections`
//! has to agree on — the detection shape, the blob encoding of a 512-dim embedding and
//! the similarity the clustering thresholds are expressed in.

/// One detected face, in the coordinates of the original image.
#[derive(Debug, Clone)]
pub struct FaceDetection {
    pub bbox: [f32; 4], // [x1, y1, x2, y2] in original image coords
    pub confidence: f32,
    #[allow(dead_code)] // Used during alignment, stored for future face crop display
    pub keypoints: [[f32; 2]; 5], // left_eye, right_eye, nose, left_mouth, right_mouth
    pub embedding: Vec<f32>, // 512-dim ArcFace embedding
}

/// Encode an embedding the way `face_detections.embedding` stores it.
pub fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Read a blob back. Trailing bytes that do not fill a `f32` are ignored.
pub fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Cosine similarity between two embeddings, `0.0` for mismatched or empty input.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_roundtrip_preserves_values() {
        let embedding = vec![0.5_f32, -1.25, 0.0, 3.0];
        let blob = embedding_to_blob(&embedding);
        assert_eq!(blob.len(), embedding.len() * 4);
        assert_eq!(blob_to_embedding(&blob), embedding);
    }

    #[test]
    fn blob_with_a_trailing_partial_word_is_tolerated() {
        let mut blob = embedding_to_blob(&[1.0_f32, 2.0]);
        blob.push(0);
        assert_eq!(blob_to_embedding(&blob), vec![1.0, 2.0]);
    }

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        let v = vec![1.0_f32, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_orthogonal_vectors_is_zero() {
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn cosine_rejects_mismatched_and_empty_input() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }
}
