-- =================================================================================
-- COMPLEX PGBENCH ANALYSIS QUERY
-- =================================================================================
-- Purpose: This query is designed to generate a complex execution plan by utilizing:
-- 1. Multiple Common Table Expressions (CTEs)
-- 2. Window Functions (RANK, AVG over partition)
-- 3. Statistical Aggregates (STDDEV)
-- 4. Conditional Logic (CASE)
-- 5. Multi-type Joins (INNER, LEFT)
--
-- Scenario:
-- We are analyzing "Branch Health" by comparing the stored branch balance against
-- the sum of its accounts. We also detect "outlier" transactions in history
-- and identify the top-performing teller per branch.
-- =================================================================================

WITH 
    -- 1. ACCOUNT AGGREGATION
    -- Force a scan on the largest table (pgbench_accounts) to sum balances per branch.
    -- This typically triggers parallel workers on large scale factors.
    Branch_Account_Totals AS (
        SELECT 
            bid, 
            COUNT(*) AS total_accounts, 
            SUM(abalance) AS calculated_sum_balance
        FROM pgbench_accounts
        GROUP BY bid
    ),

    -- 2. HISTORICAL STATISTICS
    -- Calculate statistical baselines (Average and Standard Deviation) for transactions
    -- per branch to later identify anomalies.
    Branch_History_Stats AS (
        SELECT 
            bid,
            AVG(delta) AS avg_delta,
            STDDEV(delta) AS stddev_delta
        FROM pgbench_history
        GROUP BY bid
    ),

    -- 3. OUTLIER DETECTION
    -- Join history back to the stats to find specific transactions that are
    -- > 2 standard deviations from the mean.
    History_Outliers AS (
        SELECT 
            h.bid,
            COUNT(*) AS outlier_count,
            SUM(ABS(h.delta)) AS outlier_volume
        FROM pgbench_history h
        JOIN Branch_History_Stats s ON h.bid = s.bid
        WHERE ABS(h.delta - s.avg_delta) > (2 * COALESCE(s.stddev_delta, 0))
        GROUP BY h.bid
    ),

    -- 4. TELLER RANKING (Window Functions)
    -- Rank tellers within their specific branch based on their current balance.
    -- We want to isolate the #1 teller per branch.
    Teller_Rankings AS (
        SELECT 
            tid,
            bid,
            tbalance,
            RANK() OVER (PARTITION BY bid ORDER BY tbalance DESC) as rank_in_branch,
            -- Calculate a moving average of teller balance relative to the whole branch
            tbalance - AVG(tbalance) OVER (PARTITION BY bid) as diff_from_avg_teller
        FROM pgbench_tellers
    )

-- FINAL REPORT GENERATION
SELECT 
    b.bid AS branch_id,
    'Branch ' || b.bid AS branch_name, -- Fixed: Standard pgbench table has no name column
    
    -- Discrepancy Check
    b.bbalance AS stored_balance,
    bat.calculated_sum_balance,
    (b.bbalance - bat.calculated_sum_balance) AS balance_discrepancy,

    -- Complex Case Logic for Health Status
    CASE 
        WHEN (b.bbalance - bat.calculated_sum_balance) = 0 THEN 'PERFECT_MATCH'
        WHEN ABS(b.bbalance - bat.calculated_sum_balance) < 1000 THEN 'MINOR_DRIFT'
        ELSE 'MAJOR_DISCREPANCY'
    END AS health_status,

    -- Outlier Data
    COALESCE(outliers.outlier_count, 0) AS suspicious_transactions,
    
    -- Top Teller Details
    t_rank.tid AS top_teller_id,
    t_rank.tbalance AS top_teller_balance,
    ROUND(t_rank.diff_from_avg_teller, 2) AS top_teller_lead_margin

FROM pgbench_branches b
-- Join 1: Get the account aggregates
INNER JOIN Branch_Account_Totals bat ON b.bid = bat.bid
-- Join 2: Get the outlier stats (Left join as history might be empty or clean)
LEFT JOIN History_Outliers outliers ON b.bid = outliers.bid
-- Join 3: Get the specific top ranked teller (Filtering the window function result)
LEFT JOIN Teller_Rankings t_rank ON b.bid = t_rank.bid AND t_rank.rank_in_branch = 1

WHERE 
    -- Filter to make the final set interesting (e.g., active branches)
    bat.total_accounts > 0
    
ORDER BY 
    ABS(b.bbalance - bat.calculated_sum_balance) DESC, 
    b.bid ASC;