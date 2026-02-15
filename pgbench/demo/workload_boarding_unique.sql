i-- workload_bookings_ref_random.sql

-- 1. Set a random integer range.
-- The max value x'40000' (hex) is approx 262144. Adjust '300000' to match your actual data volume.
-- pgbench -f workload_bookings_ref_random.sql -c 10 -j 2 -T 60 demo
-- 1. Random Flight ID (Adjust max value based on your DB)
\set p_flight_id random(1, 33121)

-- 2. Random Seat Row (1-50)
\set p_row random(1, 50)

-- 3. Random Seat Letter Offset (0-5, corresponding to A-F)
\set p_letter_offset random(0, 5)

BEGIN;
  SELECT ticket_no
  FROM bookings.boarding_passes
  WHERE flight_id = :p_flight_id
    -- Cast the concatenated string to text or varchar explicitly if needed,
    -- though varchar columns usually handle text comparison well.
    -- For strict correctness with the index:
    AND seat_no = (:p_row::text || chr(65 + :p_letter_offset))::varchar;
END;

