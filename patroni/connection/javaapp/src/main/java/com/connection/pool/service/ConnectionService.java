package com.connection.pool.service;

import com.connection.pool.entity.ConnectionPoolDTO;
import com.zaxxer.hikari.HikariDataSource;
import com.zaxxer.hikari.HikariPoolMXBean;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.stereotype.Service;

@Service
public class ConnectionService {

    @Autowired
    private HikariDataSource ds;

    public ConnectionPoolDTO getConnectionPoolStatus() {
        ConnectionPoolDTO connectionPoolDTO = new ConnectionPoolDTO();
        HikariPoolMXBean mxBean = ds.getHikariPoolMXBean();
        connectionPoolDTO.setActiveConnections(mxBean.getActiveConnections());
        connectionPoolDTO.setIdleConnections(mxBean.getIdleConnections());
        connectionPoolDTO.setTotalConnections(mxBean.getTotalConnections());
        connectionPoolDTO.setMaxConnections(ds.getMaximumPoolSize());
        connectionPoolDTO.setMinConnections(ds.getMinimumIdle());
        connectionPoolDTO.setPendingConnections(mxBean.getThreadsAwaitingConnection());
        return connectionPoolDTO;
    }
}
