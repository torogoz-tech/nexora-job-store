use super::KnowledgeChunker;

#[test]
fn chunks_normalized_source_text_with_stable_content_hashes() {
    let chunks = KnowledgeChunker::new(5, 1)
        .unwrap()
        .chunk("resource-a", 7, "Source title", " alpha  beta\ngamma ")
        .unwrap();

    assert_eq!(chunks.len(), 4);
    assert_eq!(chunks[0].sequence, 0);
    assert_eq!(chunks[0].content, "alpha");
    assert_eq!(
        chunks[0].id,
        "fa224bd4a6d75c80961790a2957777a9d318cd54c18608bb3ba7bc0c12c5fa3c"
    );
    assert_eq!(
        chunks[0].content_hash,
        "8ed3f6ad685b959ead7022518e1af76cd816f8e8ec7ccdda1ed4018e8f2223f8"
    );
    assert_eq!(chunks[1].sequence, 1);
    assert_eq!(chunks[1].content, "a bet");
    assert_eq!(
        chunks[1].id,
        "9d7db1c54108728119a159f02dda1af8e26701f95c120d250ec4ff3332aafd03"
    );
    assert_eq!(chunks[2].sequence, 2);
    assert_eq!(chunks[2].content, "ta ga");
    assert_eq!(chunks[3].sequence, 3);
    assert_eq!(chunks[3].content, "amma");
    assert_eq!(chunks[0].title, "Source title");
}

#[test]
fn empty_text_produces_no_chunks_and_invalid_limits_are_rejected() {
    assert!(
        KnowledgeChunker::new(5, 1)
            .unwrap()
            .chunk("resource-a", 1, "title", "  \n ")
            .unwrap()
            .is_empty()
    );
    assert!(KnowledgeChunker::new(0, 0).is_err());
    assert!(KnowledgeChunker::new(5, 5).is_err());
}
