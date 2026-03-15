* add fallback to the controllers in case no claims are injected in the controller
* issue token check if there is actually a serviceaccount in the namespace where we want to issue the token
* check for token duplicates for the create token cli command
* rework the script to store the keys and initial tokens in a file, do not reset the db all the time
* extract_apply_identities does not work for implied object infos
* the rbac cache should be decoupled from the backend
* global roles and global role bindings can only be created in the system namespace
* add validation for the role verbs or use enum for the role verbs

refactoring base functionality:
  * move the fk and schema cache to the core
  * create new handlers for apply_handler
  * we need to refactor the object preparation and validation to:
    * object preparation (unwrap lists)
    * check permissions of outer list of objects
    * check serialization
    * check schema of outer objects
    * check foreign keys and extract foreign key objects from the navigation properties
      * recursive
      * check permission of the objects extracted with the foreign keys
      * check schema of the nav props
      * check foreign keys 


checkout @dawnstore-api/src/models.rs there is a Container model, it contains the field parent_object this is a navigation property, a
  placeholder which is not written into the database, it gets filled in the get endpoint if the and
