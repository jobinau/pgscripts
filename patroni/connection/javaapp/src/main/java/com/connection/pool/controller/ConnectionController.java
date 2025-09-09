package com.connection.pool.controller;

import com.connection.pool.entity.ConnectionPoolDTO;
import com.connection.pool.service.ConnectionService;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/con")
public class ConnectionController {
    @Autowired
    private ConnectionService connectionService;

    @GetMapping
    public ConnectionPoolDTO getConnectionPoolStatus() {
        return connectionService.getConnectionPoolStatus();
    }
}
