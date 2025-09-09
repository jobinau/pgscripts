package com.connection.pool;

import com.connection.pool.service.ConnectionService;
import com.connection.pool.service.EmployeeService;
import jakarta.annotation.PostConstruct;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.context.ApplicationContext;
import org.springframework.core.task.SimpleAsyncTaskExecutor;
import org.springframework.core.task.TaskExecutor;
import org.springframework.scheduling.annotation.EnableAsync;
import org.springframework.stereotype.Component;

@Component
@EnableAsync
public class ExecutorBase {

    @Autowired
    private ApplicationContext applicationContext;

    @PostConstruct
    public void atStartup() {
        TaskExecutor taskExecutor = new SimpleAsyncTaskExecutor();
        EmployeeService employeeService = applicationContext.getBean(EmployeeService.class);
        ConnectionService connectionService = applicationContext.getBean(ConnectionService.class);
        ConnectionTask task = new ConnectionTask();
        task.setService(connectionService);
        taskExecutor.execute(task);
        for(int i=0; i<150; i++) {
            TaskRun taskRun = new TaskRun();
            taskRun.setThreadName("TaskRunner"+ i);
            taskRun.setEmployeeService(employeeService);
            taskExecutor.execute(taskRun);
        }
    }
}
