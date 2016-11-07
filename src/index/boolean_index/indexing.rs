use std::thread;
use std::io::Write;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::atomic::{Ordering, AtomicUsize};
use std::hash::Hash;
use std::collections::HashMap;


use storage::compression::VByteEncoded;

use index::boolean_index::{Result, Error, IndexingError};
use index::boolean_index::posting::Listing;
use chunked_storage::{ChunkedStorage, IndexingChunk};
use storage::Storage;

const SORT_THREADS: usize = 4;

/// Indexes a document collection for later retrieval
/// Returns the number of documents indexed
pub fn index_documents<TDocsIterator, TDocIterator, TStorage, TTerm>
    (documents: TDocsIterator,
     storage: TStorage)
     -> Result<(usize, ChunkedStorage, HashMap<TTerm, u64>)>
    where TDocsIterator: Iterator<Item = TDocIterator>,
          TDocIterator: Iterator<Item = TTerm>,
          TStorage: Storage<IndexingChunk> + 'static,
          TTerm: Ord + Hash
{
    let (merged_tx, merged_rx) = mpsc::sync_channel(64);
    let mut document_count = 0;
    let thread_sync = Arc::new(AtomicUsize::new(0));
    // Initialize and start sorting threads
    let mut chunk_tx = Vec::with_capacity(SORT_THREADS);
    let mut sort_threads = Vec::with_capacity(SORT_THREADS);
    for _ in 0..SORT_THREADS {
        let (tx, rx) = mpsc::sync_channel(4);
        chunk_tx.push(tx);
        let m_tx = merged_tx.clone();
        let loc_sync = thread_sync.clone();
        sort_threads.push(thread::spawn(|| sort_and_group_chunk(loc_sync, rx, m_tx)));
    }
    drop(merged_tx);
    let inv_index = thread::spawn(|| invert_index(merged_rx, storage));
    let mut term_ids = HashMap::new();
    let mut buffer = Vec::with_capacity(213400);
    let mut term_count = 0;
    // For every document in the collection
    let mut chunk_count = 0;
    for (doc_id, document) in documents.enumerate() {
        // Enumerate over its terms
        for (term_position, term) in document.into_iter().enumerate() {
            // Has term already been seen? Is it already in the vocabulary?
            if let Some(term_id) = term_ids.get(&term) {
                buffer.push((*term_id, doc_id as u64, term_position as u32));
                continue;
            }
            term_ids.insert(term, term_count as u64);
            buffer.push((term_count as u64, doc_id as u64, term_position as u32));
            term_count += 1;
        }
        // Term was not yet indexed. Add it
        document_count += 1;
        if document_count % 256 == 0 {
            let index = chunk_count % SORT_THREADS;
            let old_len = buffer.len();
            try!(chunk_tx[index].send((chunk_count, buffer)));
            buffer = Vec::with_capacity(old_len + old_len / 10);
            chunk_count += 1;
        }
    }
    try!(chunk_tx[chunk_count % SORT_THREADS].send((chunk_count, buffer)));
    drop(chunk_tx);
    // Join sort threads
    if sort_threads.into_iter().any(|thread| thread.join().is_err()) {
        return Err(Error::Indexing(IndexingError::ThreadPanic));
    }
    // Join invert index thread and save result
    let chunked_postings = match inv_index.join() {
        Ok(res) => try!(res),
        Err(_) => return Err(Error::Indexing(IndexingError::ThreadPanic)),
    };

    Ok((document_count, chunked_postings, term_ids))
}

/// Receives chunks of (`term_id`, `doc_id`, `position`) tripels
/// Sorts and groups them by `term_id` and `doc_id` then sends them
fn sort_and_group_chunk(sync: Arc<AtomicUsize>,
                        ids: mpsc::Receiver<(usize, Vec<(u64, u64, u32)>)>,
                        grouped_chunks: mpsc::SyncSender<Vec<(u64, Listing)>>) {

    while let Ok((id, mut chunk)) = ids.recv() {
        // Sort triples by term_id
        chunk.sort_by_key(|&(a, _, _)| a);
        let mut grouped_chunk = Vec::with_capacity(chunk.len());
        let mut last_tid = 0;
        let mut term_counter = 0;
        // Group by term_id and doc_id
        for (i, &(term_id, doc_id, pos)) in chunk.iter().enumerate() {
            // if term is the first term or different to the last term (new group)
            if last_tid < term_id || i == 0 {
                term_counter += 1;
                // Term_id has to be added
                grouped_chunk.push((term_id, vec![(doc_id, vec![pos])]));
                last_tid = term_id;
                continue;
            }
            // Term_id is already known.
            {
                let mut posting = grouped_chunk[term_counter - 1].1.last_mut().unwrap();
                // Check if last doc_id equals this doc_id
                if posting.0 == doc_id {
                    // If so only push the new position
                    posting.1.push(pos);
                    continue;
                }
            }
            // Otherwise add a whole new posting
            grouped_chunk[term_counter - 1].1.push((doc_id, vec![pos]));
        }
        // Send grouped chunk to merger thread. Make sure to send chunks in right order
        // (yes, this is a verb: https://en.wiktionary.org/wiki/grouped#English)
        loop {
            let atm = sync.load(Ordering::SeqCst);
            if atm == id {
                grouped_chunks.send(grouped_chunk).unwrap();
                sync.fetch_add(1, Ordering::SeqCst);
                break;
            }
        }
    }
}

fn invert_index<TStorage>(grouped_chunks: mpsc::Receiver<Vec<(u64, Listing)>>,
                          storage: TStorage)
                          -> Result<ChunkedStorage>
    where TStorage: Storage<IndexingChunk> + 'static
{
    let mut storage = ChunkedStorage::new(10000, Box::new(storage));
    while let Ok(chunk) = grouped_chunks.recv() {
        let threshold = storage.len();
        for (term_id, listing) in chunk {
            let uterm_id = term_id as usize;
            // Get chunk to write to or create if unknown
            let mut stor_chunk = if uterm_id < threshold {
                storage.get_current_mut(term_id)
            } else {
                storage.new_chunk(term_id)
            };
            let base_doc_id = stor_chunk.get_last_doc_id();
            let last_doc_id = try!(write_listing(listing, base_doc_id, &mut stor_chunk));
            stor_chunk.set_last_doc_id(last_doc_id);
        }
    }
    Ok(storage)
}

fn write_listing<W: Write>(listing: Listing, mut base_doc_id: u64, target: &mut W) -> Result<u64> {
    for (doc_id, positions) in listing {
        let delta_doc_id = doc_id - base_doc_id;
        base_doc_id = doc_id;
        try!(VByteEncoded::new(delta_doc_id as usize).write_to(target));
        try!(VByteEncoded::new(positions.len()).write_to(target));
        let mut last_position = 0;
        for position in positions {
            let delta_pos = position - last_position;
            last_position = position;
            try!(VByteEncoded::new(delta_pos as usize).write_to(target));
        }
    }
    Ok(base_doc_id)
}


#[cfg(test)]
mod tests {

    use std::thread;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use utils::persistence::Volatile;
    use storage::compression::VByteDecoder;
    use index::boolean_index::posting::decode_from_storage;
    use storage::RamStorage;

    #[test]
    fn basic_sorting() {
        let (trp_tx, trp_rx) = mpsc::channel();
        let (sorted_tx, sorted_rx) = mpsc::sync_channel(64);
        let sync = Arc::new(AtomicUsize::new(0));
        thread::spawn(|| super::sort_and_group_chunk(sync, trp_rx, sorted_tx));

        // (term_id, doc_id, position)
        // Document 0: "0, 0, 1"
        // Document 1: "0"
        trp_tx.send((0, vec![(0, 0, 1), (0, 0, 2), (1, 0, 3), (0, 1, 0)])).unwrap();
        assert_eq!(sorted_rx.recv().unwrap(),
                   vec![(0, vec![(0, vec![1, 2]), (1, vec![0])]), (1, vec![(0, vec![3])])]);
    }

    #[test]
    fn extended_sorting() {
        let (trp_tx, trp_rx) = mpsc::channel();
        let (sorted_tx, sorted_rx) = mpsc::sync_channel(64);
        let sync = Arc::new(AtomicUsize::new(0));
        thread::spawn(|| super::sort_and_group_chunk(sync, trp_rx, sorted_tx));

        trp_tx.send((0, (0..100).map(|i| (i, i, i as u32)).collect::<Vec<_>>())).unwrap();
        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (0..100).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());

        trp_tx.send((1, (0..100).map(|i| (i, i, i as u32)).collect::<Vec<_>>())).unwrap();
        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (0..100).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());

        trp_tx.send((2, (200..300).map(|i| (i, i, i as u32)).collect::<Vec<_>>())).unwrap();
        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (200..300).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());
    }

    #[test]
    fn multi_sorting() {
        let (sorted_tx, sorted_rx) = mpsc::sync_channel(64);
        let sync = Arc::new(AtomicUsize::new(0));
        for i in 0..2 {
            let (trp_tx, trp_rx) = mpsc::channel();
            let local_sync = sync.clone();
            let loc_tx = sorted_tx.clone();
            thread::spawn(|| super::sort_and_group_chunk(local_sync, trp_rx, loc_tx));
            trp_tx.send((i, (i as u64..100).map(|k| (k, k, k as u32)).collect::<Vec<_>>())).unwrap();
        }

        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (0..100).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());
        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (1..100).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());
    }

    #[test]
    fn multi_sorting_asymetric() {
        let (sorted_tx, sorted_rx) = mpsc::sync_channel(64);
        let sync = Arc::new(AtomicUsize::new(0));
        for i in 0..2 {
            let (trp_tx, trp_rx) = mpsc::channel();
            let local_sync = sync.clone();
            let loc_tx = sorted_tx.clone();
            thread::spawn(|| super::sort_and_group_chunk(local_sync, trp_rx, loc_tx));
            if i == 0 {
                trp_tx.send((i, (i as u64..10000).map(|k| (k, k, k as u32)).collect::<Vec<_>>())).unwrap();
            } else {
                trp_tx.send((i, (i as u64..10).map(|k| (k, k, k as u32)).collect::<Vec<_>>())).unwrap();
            }
        }

        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (0..10000).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());
        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (1..10).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());
    }

    #[test]
    fn multi_sorting_messedup() {
        let (sorted_tx, sorted_rx) = mpsc::sync_channel(64);
        let sync = Arc::new(AtomicUsize::new(0));
        for i in 0..2 {
            let (trp_tx, trp_rx) = mpsc::channel();
            let local_sync = sync.clone();
            let loc_tx = sorted_tx.clone();
            thread::spawn(|| super::sort_and_group_chunk(local_sync, trp_rx, loc_tx));
            trp_tx.send((1 - i, (i as u64..100).map(|k| (k, k, k as u32)).collect::<Vec<_>>())).unwrap();
        }

        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (1..100).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());
        let sorted = sorted_rx.recv().unwrap();
        assert_eq!(sorted,
                   (0..100).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>());
    }


    #[test]
    fn basic_inverting() {
        let (sorted_tx, sorted_rx) = mpsc::sync_channel(64);
        let result = thread::spawn(|| super::invert_index(sorted_rx, RamStorage::new()));

        sorted_tx.send((0..100).map(|i| (i, vec![(i, vec![i as u32])])).collect::<Vec<_>>()).unwrap();
        drop(sorted_tx);

        let chunked_storage = result.join().unwrap().unwrap();
        assert_eq!(chunked_storage.len(), 100);
        assert_eq!(decode_from_storage(&chunked_storage, 0).unwrap(),
                   vec![(0, vec![0u32])]);
        assert_eq!(decode_from_storage(&chunked_storage, 99).unwrap(),
                   vec![(99, vec![99u32])]);
    }

    #[test]
    fn chunk_overflowing_inverting() {
        let (sorted_tx, sorted_rx) = mpsc::sync_channel(64);
        let result = thread::spawn(|| super::invert_index(sorted_rx, RamStorage::new()));

        sorted_tx.send((0..10)
                .map(|i| (i, (i..i + 100).map(|k| (k, (0..10).collect::<Vec<_>>())).collect::<Vec<_>>()))
                .collect::<Vec<_>>())
            .unwrap();
        drop(sorted_tx);

        let chunked_storage = result.join().unwrap().unwrap();
        assert_eq!(chunked_storage.len(), 10);
        assert_eq!(decode_from_storage(&chunked_storage, 0).unwrap(),
                   (0..100).map(|k| (k, (0..10).collect::<Vec<_>>())).collect::<Vec<_>>());

    }

    #[test]
    fn overflowing_posting() {
        let (sorted_tx, sorted_rx) = mpsc::sync_channel(64);
        let result = thread::spawn(|| super::invert_index(sorted_rx, RamStorage::new()));

        sorted_tx.send((0..1)
                .map(|i| (i, (i..i + 1).map(|k| (k, (0..10000).collect::<Vec<_>>())).collect::<Vec<_>>()))
                .collect::<Vec<_>>())
            .unwrap();
        drop(sorted_tx);


        let chunked_storage = result.join().unwrap().unwrap();
        assert_eq!(chunked_storage.len(), 1);
        assert_eq!(decode_from_storage(&chunked_storage, 0).unwrap(),
                   (0..1).map(|k| (k, (0..10000).collect::<Vec<_>>())).collect::<Vec<_>>());
    }


    #[test]
    fn write_listing_basic() {
        let listing = vec![(0, vec![0, 1, 2]), (1, vec![1, 2, 3])];
        let mut bytes = Vec::new();
        super::write_listing(listing, 0, &mut bytes).unwrap();
        let data = VByteDecoder::new(bytes.as_slice()).collect::<Vec<_>>();
        assert_eq!(data, vec![0, 3, 0, 1, 1, 1, 3, 1, 1, 1]);
    }

    #[test]
    fn write_listing_real_data() {
        let listing = vec![(0, vec![16]),
                           (1, vec![12, 25]),
                           (2, vec![14, 21, 44]),
                           (3, vec![18]),
                           (4, vec![28, 38]),
                           (6, vec![11]),
                           (7, vec![19, 45]),
                           (8, vec![23]),
                           (9, vec![32]),
                           (10, vec![2, 4]),
                           (11, vec![18, 27]),
                           (12, vec![19]),
                           (13, vec![12, 29]),
                           (14, vec![33]),
                           (16, vec![3]),
                           (20, vec![32]),
                           (22, vec![2, 22, 29]),
                           (23, vec![32]),
                           (24, vec![4, 25]),
                           (25, vec![11]),
                           (27, vec![42]),
                           (28, vec![8, 14, 46]),
                           (29, vec![48]),
                           (30, vec![23]),
                           (31, vec![36]),
                           (33, vec![1]),
                           (36, vec![9]),
                           (37, vec![30]),
                           (39, vec![21]),
                           (43, vec![7, 9, 18]),
                           (44, vec![34]),
                           (45, vec![23]),
                           (46, vec![17, 35]),
                           (47, vec![33]),
                           (48, vec![19]),
                           (49, vec![1])];
        let mut bytes = Vec::new();
        super::write_listing(listing, 0, &mut bytes).unwrap();
        let data = VByteDecoder::new(bytes.as_slice()).collect::<Vec<_>>();
        assert_eq!(data[..12].to_vec(), vec![0, 1, 16, 1, 2, 12, 13, 1, 3, 14, 7, 23]);
    }



}
