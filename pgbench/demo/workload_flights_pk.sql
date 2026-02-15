-- workload_flights_pk.sql
-- Adjust the range (1, 33121) based on your SELECT max(flight_id) result
-- pgbench -f workload_flights_pk.sql -c 10 -j 2 -T 60 demo
\set fid random(1, 33121)

BEGIN;
  SELECT flight_no, scheduled_departure, status 
  FROM bookings.flights 
  WHERE flight_id = :fid;
END;
