# Using Keepalived Instead of HAProxy
This method uses VIP failover between PostgreSQL nodes as a method to route the connection, Than using HAProxy as an external connection router.
This eliminates an additional network hop and TCP stack

### Advantages
1. Clients / Application can directly connect to the database host, instead of a proxy
2. No additional network hops and delays. This could result in faster performance.
3. Client IPs will be visible at the PostgreSQL side for monitoring.
4. Useful if all the connections are to be routed to the Primary
5. Useful for one primary and one standby configuration

### Disadvantages
1. No load balancing for read-only connections, if there are multiple standby nodes
2. Keepalived (VIP failover) is not proven for its stability in Patroni clusters
3. Keepalived is prone to network partitioning problems.