package com.connection.pool.controller;

import com.connection.pool.entity.Employee;
import com.connection.pool.service.EmployeeService;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.web.bind.annotation.*;

import java.util.List;

@RestController
@RequestMapping("/emps")
public class EmployeeController {

    @Autowired
    private EmployeeService employeeService;

    @GetMapping
    public List<Employee> getAllEmployees() {
        for(int i =0; i<5; i++){
            List<Employee> list = employeeService.getAllEmps();
            for (Employee employee : list) {
                System.out.println(employee.getId() + employee.getName());
                try {
                    Thread.sleep(500);
                } catch (InterruptedException e) {
                    throw new RuntimeException(e);
                }
            }
        }
        return employeeService.getAllEmps();
    }

    @GetMapping("/{id}")
    public Employee getEmpById(@PathVariable Long id) {
        return employeeService.getEmployeeById(id);
    }

    @PostMapping
    public Employee createEmployee(@RequestBody Employee lEmp) {
        return employeeService.saveEmployee(lEmp);
    }

    @PutMapping("/{id}")
    public Employee updateEmployee(@PathVariable Long id, @RequestBody Employee employee) {
        Employee existingEmp = employeeService.getEmployeeById(id);
        if (existingEmp != null) {
           // existingEmp.setName(employee.getName());
            return employeeService.saveEmployee(existingEmp);
        }
        return null;
    }

    @DeleteMapping("/{id}")
    public void deleteEmp(@PathVariable Long id) {
        employeeService.deleteEmployee(id);
    }
}
