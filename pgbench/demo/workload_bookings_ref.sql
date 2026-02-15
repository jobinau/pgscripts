-- workload_bookings_ref_random.sql

-- 1. Set a random integer range.
-- The max value x'40000' (hex) is approx 262144. Adjust '300000' to match your actual data volume.
-- pgbench -f workload_bookings_ref.sql -c 10 -j 2 -T 60 demo
\set r_id random(1, 300000)

BEGIN;
  SELECT book_date, total_amount
  FROM bookings.bookings
  -- Explicitly cast the generated text to bpchar (character) to match the index
  WHERE book_ref = upper(lpad(to_hex(:r_id), 6, '0'))::bpchar;
END;

