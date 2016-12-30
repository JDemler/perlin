use std::hash::Hash;
use std::collections::HashMap;

use page_manager::RamPageCache;
use index::listing::Listing;
use index::posting::{DocId, Posting};
use index::vocabulary::{Vocabulary, TermId};

pub mod vocabulary;
pub mod posting;
mod listing;


pub struct Index<TTerm> {
    page_manager: RamPageCache,
    listings: Vec<(TermId, Listing)>,
    vocabulary: HashMap<TTerm, TermId>,
    doc_count: u64,
}


impl<TTerm> Index<TTerm>
    where TTerm: Hash + Ord
{
    pub fn new(page_manager: RamPageCache) -> Self {
        Index {
            page_manager: page_manager,
            listings: Vec::new(),
            vocabulary: HashMap::new(),
            doc_count: 0,
        }

    }

    pub fn index_collection<TDocIter, TDocsIter>(&mut self, collection: TDocsIter) -> Vec<DocId>
        where TDocIter: Iterator<Item = TTerm>,
              TDocsIter: Iterator<Item = TDocIter>
    {
        let mut buff = Vec::new();
        for doc in collection {
            let doc_id = DocId(self.doc_count);
            self.doc_count += 1;
            for term in doc {
                let term_id = self.vocabulary.get_or_add(term);
                buff.push((term_id, doc_id));
            }
        }
        buff.sort_by_key(|&(tid, _)| tid);
        buff.dedup();
        let mut grouped_buff = Vec::with_capacity(buff.len());
        let mut last_tid = TermId(0);
        let mut term_counter = 0;
        for (i, &(term_id, doc_id)) in buff.iter().enumerate() {
            // if term is the first term or different to the last term (new group)
            if last_tid < term_id || i == 0 {
                term_counter += 1;
                // Term_id has to be added
                grouped_buff.push((term_id, vec![Posting(doc_id)]));
                last_tid = term_id;
                continue;
            }          
            // Otherwise add a whole new posting
            grouped_buff[term_counter - 1].1.push(Posting(doc_id));
        }
        for (term_id, postings) in grouped_buff {
             let index = match self.listings.binary_search_by_key(&term_id, |&(t_id, _)| t_id) {
                Ok(index) => index,
                Err(index) => {
                    self.listings.insert(index, (term_id, Listing::new()));
                    index
                }
            };
            self.listings[index].1.add(&postings, &mut self.page_manager);
        }
        self.commit();
        vec![]
    }

    pub fn index_document<TIter>(&mut self, document: TIter) -> DocId
        where TIter: Iterator<Item = TTerm>
    {
        let doc_id = DocId(self.doc_count);
        self.doc_count += 1;
        let mut buff = Vec::new();
        for term in document {
            let term_id = self.vocabulary.get_or_add(term);
            buff.push(term_id);
        }
        buff.sort();
        buff.dedup();
        for term_id in buff {
            // get or add listing
            let index = match self.listings.binary_search_by_key(&term_id, |&(t_id, _)| t_id) {
                Ok(index) => index,
                Err(index) => {
                    self.listings.insert(index, (term_id, Listing::new()));
                    index
                }
            };
            self.listings[index].1.add(&[Posting(doc_id)], &mut self.page_manager);
        }
        doc_id
    }

    pub fn commit(&mut self) {
        //We iterate over the listings in reverse here because listing.commit() causes
        //a remove in the ram_page_manager.construction cache which is a Vec.
        //Vec.remove is O(n-i).
        for listing in self.listings.iter_mut().rev() {
            listing.1.commit(&mut self.page_manager);
        }
    }

    pub fn query_atom(&self, atom: &TTerm) -> Vec<Posting> {
        if let Some(term_id) = self.vocabulary.get(atom) {
            if let Ok(index) = self.listings.binary_search_by_key(term_id, |&(t_id, _)| t_id) {
                return self.listings[index].1.posting_iter(&self.page_manager).collect::<Vec<_>>();
            }
        }
        vec![]
    }
}


#[cfg(test)]
mod tests {

    use test_utils::create_test_dir;
    use super::Index;
    use index::posting::{Posting, DocId};
    use page_manager::{FsPageManager, RamPageCache};

    fn new_cache(name: &str) -> RamPageCache {
        let path = &create_test_dir(format!("index/{}", name).as_str());
        let pmgr = FsPageManager::new(&path.join("pages.bin"));
        RamPageCache::new(pmgr)
    }

    #[test]
    fn basic_indexing() {
        let cache = new_cache("basic_indexing");
        let mut index = Index::<usize>::new(cache);

        assert_eq!(index.index_document((0..2000)), DocId(0));
        assert_eq!(index.index_document((2000..4000)), DocId(1));
        assert_eq!(index.index_document((500..600)), DocId(2));
        index.commit();

        assert_eq!(index.query_atom(&0), vec![Posting(DocId(0))]);
    }

    #[test]
    fn extended_indexing() {
        let cache = new_cache("extended_indexing");
        let mut index = Index::<usize>::new(cache);
        println!("index");
        for i in 0..200 {
            assert_eq!(index.index_document((i..i + 200)), DocId(i as u64));
        }
        println!("commit");
        index.commit();

        assert_eq!(index.query_atom(&0), vec![Posting(DocId(0))]);
        assert_eq!(index.query_atom(&99),
                   (0..100).map(|i| Posting(DocId(i))).collect::<Vec<_>>());
    }

    #[test]
    fn collection_indexing() {
        let cache = new_cache("collection_indexing");
        let mut index = Index::<usize>::new(cache);
        assert_eq!(index.index_collection((0..200).map(|i| (i..i+200))), vec![]);

        assert_eq!(index.query_atom(&0), vec![Posting(DocId(0))]);
        assert_eq!(index.query_atom(&99),
                   (0..100).map(|i| Posting(DocId(i))).collect::<Vec<_>>());
    }

    #[test]
    fn mutable_index() {
        let cache = new_cache("mutable_index");
        let mut index = Index::<usize>::new(cache);
        for i in 0..200 {
            assert_eq!(index.index_document((i..i + 200)), DocId(i as u64));
        }
        index.commit();

        assert_eq!(index.query_atom(&0), vec![Posting(DocId(0))]);
        assert_eq!(index.query_atom(&99),
                   (0..100).map(|i| Posting(DocId(i))).collect::<Vec<_>>());
        assert_eq!(index.index_document(0..400), DocId(200));
        index.commit();
        assert_eq!(index.query_atom(&0),
                   vec![Posting(DocId(0)), Posting(DocId(200))]);
    }
}
