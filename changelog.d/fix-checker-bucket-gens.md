### Fixed

- **The checker reads the allocation btrees' bucket generations (#163).**
  A bucket whose generation in `bucket_gens` is not the one its alloc key
  holds is reported as `incorrect_gen_in_bucket_gens`, as the reference's
  checker reports "incorrect gen in bucket_gens". The checker judged no
  allocation btree before.
