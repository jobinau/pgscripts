package com.connection.pool;

import com.connection.pool.entity.Employee;
import com.connection.pool.service.EmployeeService;
import org.springframework.context.annotation.Scope;
import org.springframework.stereotype.Component;

import java.util.List;

@Component
@Scope("application")
public class TaskRun implements Runnable {
    private String threadName;

    EmployeeService employeeService;

    public String getThreadName() {
        return threadName;
    }

    public void setThreadName(String threadName) {
        this.threadName = threadName;
    }

    public EmployeeService getEmployeeService() {
        return employeeService;
    }

    public void setEmployeeService(EmployeeService employeeService) {
        this.employeeService = employeeService;
    }

    @Override
    public void run() {
        System.out.println(threadName + " Started");
        longBackgorund();
    }

    protected void longBackgorund() {
        System.out.println("Starting execution by " + threadName);
        int i=1;
        while (true) {
            //System.out.println("Starting execution by " + threadName);
            Employee emp = new Employee();
            emp.setName("Name" + i);
            i++;
            try {
                employeeService.saveEmployee(emp);
            }catch(Exception e){
                e.printStackTrace();
            }
            List<Employee> empList = employeeService.getAllEmps();
            //empList.forEach(e -> System.out.println(threadName + " printing Employee Details: Emp ID " + e.getId()));
            try {
                Thread.sleep(1000);
            } catch (InterruptedException e) {
                e.printStackTrace();
            }
        }
    }
}
