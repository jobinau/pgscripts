# Connection pooler using deadpool-postgres.
hundreds of connections will be picking a connection from the pool and executing the queries
## Build
```
cargo build --release
```
## Run
```
time target/release/rustpg  > /dev/shm/out.txt
```

### Check the threads running on CPU core
```
ps -L -o pid,tid,psr,pcpu,comm -p `pidof rustpg`
```

### Sample Schema usedA
```
CREATE TABLE emp (
    emp_id BIGSERIAL PRIMARY KEY, -- Maps to i64 in Rust
    first_name VARCHAR(50) NOT NULL,
    last_name VARCHAR(50) NOT NULL,
    email VARCHAR(100) UNIQUE,
    hire_date DATE DEFAULT CURRENT_DATE,
    job_title VARCHAR(50),
    department VARCHAR(50),
    salary NUMERIC(10, 2),
    manager_id BIGINT REFERENCES emp(emp_id) -- Must match BIGINT
);
```


```
INSERT INTO emp (first_name, last_name, email, hire_date, job_title, department, salary) VALUES
('James', 'Smith', 'james.smith@company.com', '2020-01-15', 'Engineering Manager', 'Engineering', 125000),
('Maria', 'Garcia', 'maria.garcia@company.com', '2020-02-10', 'Senior Developer', 'Engineering', 110000),
('Robert', 'Johnson', 'robert.johnson@company.com', '2020-03-05', 'Sales Director', 'Sales', 95000),
('Linda', 'Williams', 'linda.williams@company.com', '2020-04-12', 'HR Specialist', 'HR', 65000),
('Michael', 'Brown', 'michael.brown@company.com', '2020-05-20', 'Marketing Lead', 'Marketing', 88000),
('Elizabeth', 'Jones', 'elizabeth.jones@company.com', '2020-06-18', 'Support Analyst', 'Support', 55000),
('David', 'Miller', 'david.miller@company.com', '2020-07-22', 'DevOps Engineer', 'Engineering', 105000),
('Jennifer', 'Davis', 'jennifer.davis@company.com', '2020-08-30', 'Account Executive', 'Sales', 72000),
('William', 'Rodriguez', 'william.rodriguez@company.com', '2020-09-14', 'Content Strategist', 'Marketing', 67000),
('Susan', 'Martinez', 'susan.martinez@company.com', '2020-10-05', 'QA Engineer', 'Engineering', 82000),
('Christopher', 'Hernandez', 'chris.h@company.com', '2021-01-12', 'Full Stack Developer', 'Engineering', 98000),
('Jessica', 'Lopez', 'jessica.l@company.com', '2021-02-14', 'Recruiter', 'HR', 62000),
('Matthew', 'Gonzalez', 'matt.g@company.com', '2021-03-10', 'Data Scientist', 'Engineering', 115000),
('Sarah', 'Wilson', 'sarah.w@company.com', '2021-04-22', 'Social Media Manager', 'Marketing', 58000),
('Daniel', 'Anderson', 'dan.a@company.com', '2021-05-01', 'Sales Manager', 'Sales', 90000),
('Karen', 'Thomas', 'karen.t@company.com', '2021-06-15', 'UX Designer', 'Engineering', 89000),
('Nancy', 'Taylor', 'nancy.t@company.com', '2021-07-19', 'Customer Success', 'Support', 52000),
('Paul', 'Moore', 'paul.m@company.com', '2021-08-25', 'Backend Developer', 'Engineering', 94000),
('Lisa', 'Jackson', 'lisa.j@company.com', '2021-09-30', 'SEO Specialist', 'Marketing', 63000),
('Kevin', 'Martin', 'kevin.m@company.com', '2021-10-12', 'Frontend Developer', 'Engineering', 87000);

-- Generating additional generic rows to hit the 100 mark
INSERT INTO emp (first_name, last_name, email, hire_date, job_title, department, salary)
SELECT 
    'User' || i, 
    'Lastname' || i, 
    'user' || i || '@company.com', 
    '2022-01-01'::date + (i * interval '1 day'),
    CASE WHEN i % 3 = 0 THEN 'Developer' WHEN i % 3 = 1 THEN 'Analyst' ELSE 'Associate' END,
    CASE WHEN i % 4 = 0 THEN 'Engineering' WHEN i % 4 = 1 THEN 'Sales' WHEN i % 4 = 2 THEN 'Marketing' ELSE 'HR' END,
    40000 + (random() * 60000)::numeric(10,2)
FROM generate_series(21, 100) AS i;
```
