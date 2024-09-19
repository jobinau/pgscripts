#!/usr/bin/ruby
require 'pg'

# Replace with your database credentials
host = "your_host"
database = "your_database"
user = "your_user"
password = "your_password"

begin
  conn = PG.connect("host=localhost dbname=postgres user=user1 password=user1" )

  # Replace with your desired query
  query = "SELECT * FROM t1"

  # Execute the query
  results = conn.exec(query)

  # Print the results (optional)
  results.each do |row|
    puts row
  end

rescue PG::Error => e
  puts "Error: #{e.message}"

ensure
  # Close the connection
  conn.close if conn
end
