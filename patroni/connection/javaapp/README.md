## Install Java runtime and development tools
```
##java-21-openjdk-21
sudo dnf install java-21-openjdk
##java-21-openjdk-devel
sudo dnf install java-21-openjdk-devel
```

## Set Enviroment Variables
Following is just example, Please set the values according to your environment
```
export JAVA_HOME=/usr/lib/jvm/java-21-openjdk-21.0.8.0.9-1.el8.x86_64
export MAVEN_HOME=/home/postgres/apache-maven-3.9.11
export M2_HOME=/home/postgres/apache-maven-3.9.11
export MAVEN_OPTS="-Djansi.tmpdir=/home/postgres/mvnjansi"
export PATH=$JAVA_HOME/bin:$PGBIN:$PATH:$MAVEN_HOME/bin
```

### Build the application
```
mvn clean install
```

### Run the application
```
java -jar target/ModelProject-1.0-SNAPSHOT.jar
```
