package com.connection.pool.entity;

public class ConnectionPoolDTO {
    private int activeConnections;
    private int idleConnections;
    private int minConnections;
    private int maxConnections;
    private int totalConnections;
    private int pendingConnections;

    public int getPendingConnections() {
        return pendingConnections;
    }

    public void setPendingConnections(int pendingConnections) {
        this.pendingConnections = pendingConnections;
    }

    public int getActiveConnections() {
        return activeConnections;
    }

    public void setActiveConnections(int activeConnections) {
        this.activeConnections = activeConnections;
    }

    public int getIdleConnections() {
        return idleConnections;
    }

    public void setIdleConnections(int idleConnections) {
        this.idleConnections = idleConnections;
    }

    public int getMinConnections() {
        return minConnections;
    }

    public void setMinConnections(int minConnections) {
        this.minConnections = minConnections;
    }

    public int getMaxConnections() {
        return maxConnections;
    }

    public void setMaxConnections(int maxConnections) {
        this.maxConnections = maxConnections;
    }

    public int getTotalConnections() {
        return totalConnections;
    }

    public void setTotalConnections(int totalConnections) {
        this.totalConnections = totalConnections;
    }
}
