package com.connection.pool;

import com.connection.pool.entity.ConnectionPoolDTO;
import com.connection.pool.service.ConnectionService;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.context.annotation.Scope;
import org.springframework.stereotype.Component;

@Component
@Scope("application")
public class ConnectionTask implements Runnable{
    Logger logger = LoggerFactory.getLogger(ConnectionTask.class);
    private ConnectionService service;

    public ConnectionService getService() {
        return service;
    }

    public void setService(ConnectionService service) {
        this.service = service;
    }

    @Override
    public void run() {
        logger.debug("Starting Connection Task");
        while(true){
            ConnectionPoolDTO dto = service.getConnectionPoolStatus();
            logger.info("--- Checking Connection Status ----");
            logger.info("Active Connections ="  + dto.getActiveConnections());
            logger.info("Idle Connections ="  + dto.getIdleConnections());
            logger.info("Total Connections ="  + dto.getTotalConnections());
            logger.info("Max Connections ="  + dto.getMaxConnections());
            logger.info("Min Connections ="  + dto.getMinConnections());
            logger.info("Pending Connections ="  + dto.getPendingConnections());
            logger.info("-----------------------------");
            try {
                Thread.sleep(100);
            } catch (InterruptedException e) {
                e.printStackTrace();
            }
        }
    }
}
